import { h, createSignal, createList, createShow } from "@getforma/core";
// v9's no-JS vocabulary tables. They are DECLARED in MapPage.ts because the
// compiler expands `...CONST.map(…)` spreads at build time from the root
// *Page file's constants only (ledger #17, explained there); this import is
// the runtime half of that single source. The cycle is inert — nothing here
// reads them before MapIsland() runs.
import {
  KEYS_LETTER,
  KEYS_DIGIT,
  KEYS_FN,
  KEYS_NUMPAD,
  KEYS_ARROW,
  KEYS_NAV,
  KEYS_EDIT,
  KEYS_MOD,
  KEYS_SYMBOL,
  KEYS_MEDIA,
  KEYS_OEM,
  FUNCTIONS,
} from "./MapPage";

// THE MAPPER (v5): click a control on the pad art, press the panel key,
// binding saved. One island, same architecture as StatusIsland.ts — the
// module-level signals are the page's live state store AND the compile-time
// slot table (compiler 0.3.1 walks island component files for signal scopes,
// so the twin declarations MapPage.ts used to carry are gone — ledger #9,
// adopted 2026-08-06). `applyMap` seeds them from the server's payload block
// BEFORE adoption (ledger #5) and the 2 s /api/map poller (map.ts) keeps
// rewriting them. Derivations here MIRROR
// crates/ksx-studio/src/render_map.rs (server derives for the SSR first
// paint, this file re-derives per poll); the Rust unit tests pin that side —
// keep both in sync when either changes.
//
// Layout per PadForge's lesson (docs/research/padforge-ui-lessons.md):
// chrome minimal, CONTROLLER HUGE. The art (Gamepad-Asset-Pack, MIT, by
// AL2009man — vendored + recolored by build.mjs, see ../art/README.md) fills
// the bottom of a fixed-aspect stage; the top band holds the LB/RB/LT/RT
// chips, stacked trigger-over-bumper and anchored to the body silhouette.
// Every mappable control is a positioned hit-zone <button data-fn=…> from
// the ZONES tables below (authored from ../art/extents.mjs output — the
// PadForge rule: derive layout from art with a script).
//
// v7 — each zone wears its own IDENTITY (Victor: "I can see G is mapped to A
// but I can't see the A xbox button"). The vendored art draws no letters, so
// the zone renders the control's name itself in the canonical colours (A
// green / B red / X blue / Y amber; ✕ ○ △ □ in the Sony hues), with the bound
// key as the small mono tag underneath. Unbound controls still show their
// identity — the pad reads like a controller with nothing mapped at all.
// The bindings LEGEND below the stage is the second reader: the same identity
// glyph, the group prefix, the key, and FEATURE 3's "also A · B" shared-key
// badge; a row click IS the zone click (same data-fn delegation). A shared
// hover signal (`setHot`) cross-highlights zone ↔ legend row, and the
// selection Set does the same for multi-select. Interaction lives in map.ts
// (event delegation, so list reconcile keeps everything wired).

// ── Wire types: serde field names from ksx-studio {snapshot,control}.rs ────

export interface MapperSlot {
  number: number;
  persona: string;
  persona_label: string;
  preset: string;
  keyboard: string;
  bindings: Record<string, string[]>;
  /** Newest timestamped backup label, or null when there is none. */
  backup: string | null;
  /** AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3): canonical function name → the
   *  rate it auto-fires at, as authored. Keyed by FUNCTION because that is
   *  what turbo is a property of — several keys on one control share ONE
   *  clock. Absent for every preset written before turbo existed. */
  turbo?: Record<string, number>;
  /** This slot says `macros = "off"` — the TOURNAMENT SWITCH. Every macro of
   *  its preset is silenced whatever each one's own `enabled` says, and
   *  nothing is deleted. */
  macros_off?: boolean;
}

export interface MapperSnapshot {
  generated_at: string;
  source: string;
  config_root: string;
  slots: MapperSlot[];
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  /** games.toml profile the daemon is pointed at — what Resume restarts. */
  profile: string | null;
}

export interface LearnView {
  ok: boolean;
  state: string;
  remaining_ms: number | null;
  device: string | null;
  key: string | null;
  error: string | null;
}

/** One `[macros.<name>]` step, in the shape the FILE spells it — `ms` and
 *  `frames` kept apart so a sequence authored in frames round-trips as frames
 *  (docs/INPUT-TRANSFORMS.md §1c). */
export interface MacroStepView {
  hold: string[];
  ms: number | null;
  frames: number | null;
  allow_short: boolean;
}

export interface MacroView {
  name: string;
  steps: MacroStepView[];
  /** "finish" | "abort" */
  on_release: string;
  /** "ignore" | "restart" */
  retrigger: string;
  /** "none" | "any-input" | "opposing" */
  interrupt: string;
  /** "once" | "while-held" | "turbo" — what the END of a run does while the
   *  trigger is still held. */
  repeat: string;
  /** The turbo rate as AUTHORED. Exactly one of these two in a valid file:
   *  `turbo_hz` is how a player says it, `gap_ms` is how a frame-counter does,
   *  and the editor keeps whichever the file used rather than converting
   *  behind the author's back. */
  turbo_hz: number | null;
  gap_ms: number | null;
  /** Key names that START this macro. */
  triggers: string[];
  /** This macro is `enabled = false`: it keeps its steps AND its trigger row
   *  and never runs. Said as the negative so an older payload (no field)
   *  reads as the ordinary case. */
  disabled?: boolean;
}

export interface MacroSnapshot {
  /** The provider read a preset at all. `false` is NOT "this preset has no
   *  macros" — the editor says which, because only one of them is a fact
   *  about the user's file. */
  available: boolean;
  reason: string;
  preset: string;
  macros: MacroView[];
}

/** What GET /api/map serves and what the island props carry — `MapPayload`
 *  in snapshot.rs; parity pinned there. */
export interface MapPayload {
  mapper: MapperSnapshot;
  session: SessionView;
  learn: LearnView;
  selected: number;
  macros: MacroSnapshot;
  /** Which macro the SSR paint chose (`/map?macro=NAME`). */
  macro_selected: string;
}

/** v11's grid rows. One list item per STEP: its number, its duration in the
 *  unit it was authored in, the inline amber flag, and the five step verbs
 *  (each a bare `param.field` attribute — ledger #11). */
interface MacroRow {
  n: string;
  cls: string;
  /** The duration as WORDS — "50 ms", "3 fr · 50 ms". The no-JS readout; with
   *  JavaScript the editable box beside it takes over (see `durval`). */
  dur: string;
  durtitle: string;
  /** FIX 2 — THE DURATION, INLINE AND EDITABLE ON EVERY ROW. The number in the
   *  unit this step was authored in, the row index the box writes to, the
   *  box's own warn class, and the unit toggle beside it. There is no
   *  select-a-step-first mode any more: a time is edited where it is read. */
  durval: string;
  durrow: string;
  durcls: string;
  /** "ms" / "fr" — the unit BUTTON's label, because a `<select>` inside a list
   *  item cannot be given its value by an attribute binding (map.ts has to
   *  write every one of those by hand after each poll). A two-state button is
   *  its own readout. */
  unit: string;
  unitact: string;
  unittitle: string;
  /** The plain-language readout of everything this row holds — "D-pad ↘ + A",
   *  "(nothing — neutral gap)". A diagonal reads as ONE control, because that
   *  is what the player picked and what the player means. */
  hold: string;
  /** `machold` / `machold both` / `machold none` — the accent that says "this
   *  row holds more than one control", counted over PRESENTED controls. */
  holdcls: string;
  /** v16, THE LEDGER: every diagonal on this row spelled as the pair the file
   *  stores — `↘ = dpad.down + dpad.right`. The lens is only honest if the
   *  storage is visible without opening the TOML. Empty = no diagonal. */
  exp: string;
  expcls: string;
  /** Short enough to always fit; `warntitle` carries the whole sentence. */
  warn: string;
  warntitle: string;
  warncls: string;
  selact: string;
  upact: string;
  dnact: string;
  iaact: string;
  ibact: string;
  delact: string;
  /** FIX 1 — the ✕ on the LAST REMAINING step clears its holds instead of
   *  removing the row, so the editor cannot construct the zero-step macro
   *  `mapping::save_macro` refuses. The title says which one it is doing. */
  deltitle: string;
  upcls: string;
  dncls: string;
}

/** One cell of the flat `steps × controls` matrix. Flat because a
 *  `createList` inside a list item has no seam (ledger #17's neighbour), so
 *  the matrix is one list laid out by a 25-column CSS grid. */
interface MacroCell {
  cls: string;
  /** `stepIndex|function` — what the click delegation toggles. */
  cell: string;
  mark: string;
  title: string;
}

interface MacroCol {
  /** The cell token: a function name, or `diag:<mech>:<diag>` for a pick. */
  fn: string;
  id: string;
  idcls: string;
  title: string;
}

/** v16: one cell of the GROUP BAND above the glyph row — `label` spanning
 *  `cls`'s `g<N>` columns. Without it the header carries three identical
 *  `↑ ↖ ← ↙ ↓ ↘ → ↗` runs told apart only by a tooltip. */
interface MacroGroup {
  label: string;
  cls: string;
}

interface MacroTab {
  name: string;
  label: string;
  cls: string;
  href: string;
}

export interface BindConflict {
  scope: string;
  preset: string;
  function: string;
  profile: string | null;
  slot: number | null;
}

export interface BindOutcome {
  ok: boolean;
  message: string | null;
  error: string | null;
  code: string | null;
  conflicts: BindConflict[];
  reloaded: boolean;
}

/** What `POST /api/macro/save` answers — `MacroOutcome` in control.rs. One
 *  whole `[macros.<name>]` table in, one answer out: `problems` are a
 *  refusal's rows, `warnings` are the advisories a SUCCESSFUL write still has
 *  to say out loud (a step the engine raised), and `backup` names the
 *  timestamped copy the daemon took before writing. */
export interface MacroOutcome {
  ok: boolean;
  message: string | null;
  error: string | null;
  code: string | null;
  problems: string[];
  warnings: string[];
  deleted: boolean;
  /** Does the table RUN now? */
  enabled?: boolean;
  /** This write moved ONLY the enabled flag. */
  toggled?: boolean;
  backup: string | null;
  reloaded: boolean;
}

interface SlotTab {
  num: string;
  label: string;
  cls: string;
  /** "P1" — the rail chip and the table's first column. */
  player: string;
  /** The preset FILE this slot binds, e.g. "player1". */
  preset: string;
  /** Human persona label, e.g. "Xbox 360". */
  pad: string;
  /** Keyboard alias or hardware id; "(any)" when unassigned. */
  kbd: string;
  /** The management table's row class — "strow" / "strow on". */
  rowcls: string;
  /** v9: the tab is an ANCHOR, so slot switching works with JS off —
   *  `/map?slot=N` is a route the server already understands. map.ts still
   *  intercepts the click and switches in place. */
  href: string;
}

interface ZoneRow {
  fn: string;
  cls: string;
  /** FEATURE 1: the control's own name, drawn ON the art ("A", "✕", "LB"). */
  id: string;
  /** Identity palette class — `zid id-xa`, `zid id-pc`, … */
  idcls: string;
  style: string;
  title: string;
  /// The on-zone binding tag ("" for unbound — CSS hides the empty pill).
  tag: string;
}

interface LegendRow {
  fn: string;
  /** group + identity, for the tooltip and for shared-key badges. */
  label: string;
  /** The identity glyph alone, styled as the button. */
  id: string;
  idcls: string;
  /** "LS " / "RS " / "D-pad " / "" — the disambiguating prefix. */
  group: string;
  key: string;
  cls: string;
  title: string;
  /** FEATURE 3: "also A · B" when this key drives other controls too. */
  share: string;
  sharetitle: string;
  /** "✕" on a bound row of a live page, "" otherwise (CSS hides empty).
   *  This one clears the CONTROL — every key at once. */
  clear: string;
  cleartitle: string;
  /** v10, MANY KEYS → ONE CONTROL: up to KEY_CHIPS fixed key chips, each with
   *  its own ✕ that removes JUST that key. Fixed fields rather than a nested
   *  list because a `createList` inside a list item has no seam (ledger #17's
   *  neighbour); the tail is summarized in `kmore`, and the row title always
   *  names every key. `k1rm` is the `data-rmkey` payload, `function|KEY`. */
  k1: string;
  k1cls: string;
  k1xcls: string;
  k1rm: string;
  k1title: string;
  k2: string;
  k2cls: string;
  k2xcls: string;
  k2rm: string;
  k2title: string;
  k3: string;
  k3cls: string;
  k3xcls: string;
  k3rm: string;
  k3title: string;
  /** "+2" when more keys exist than there are chips, "" otherwise. */
  kmore: string;
  kmorecls: string;
  kmoretitle: string;
  /** The row form's two v10 submits: append the picked key, or take just that
   *  one away. */
  addtitle: string;
  rmtitle: string;
  /** v9, the no-JS write path. The row's own <form> posts these: the slot
   *  number (the server resolves the preset from it — a form never has to be
   *  trusted with a preset name) and the function. `bindcls` carries the
   *  inert look when nothing can be written, `bindtitle` names the control
   *  for the select's accessible name. */
  slot: string;
  bindcls: string;
  bindtitle: string;
  /** AUTO-FIRE (§3). `turbo` is the badge — the EFFECTIVE rate, because a
   *  press and a release must each survive a 60 Hz poll and a badge echoing an
   *  undeliverable number back would be the page lying on the file's behalf.
   *  `turboval` seeds the row form's box (no-JS path). Empty = no turbo, and
   *  CSS hides the badge rather than the row changing shape. */
  turbo: string;
  turbotitle: string;
  turboval: string;
}

/** One toast in the stack (v8). Every field is a BARE per-item read in the
 *  item body — ledger #11/#15 — so the whole stack costs ZERO new shows: the
 *  Undo button is hidden by a class string (`… off`), never by a nested show
 *  and never by `:empty` (which cannot work on a slot-rendered node). */
interface ToastRow {
  id: string;
  /** "toast toast-ok" | "toast toast-warn" | "toast toast-err". */
  cls: string;
  /** The plain sentence: what just happened. */
  text: string;
  /** "btn btn-undo" while this toast can still be undone, "… off" once it
   *  cannot (not undoable, mid-undo, or already undone). */
  undocls: string;
  undotitle: string;
  dismisstitle: string;
}

// ── Zone tables — MIRROR of render_map.rs ZONE_XBOX / ZONE_DS4 ────────────
// [fn, label, cx, cy, w, h, kind]; stage-percent boxes, art bottom-aligned
// at 86% stage height (ART_SHARE). Rects are pairwise DISJOINT (pinned by
// render_map.rs `zone_tables_cover_every_mappable_function`): face buttons
// sized to the drawn circles, dpad arrows to the drawn cross, and the four
// stick-direction wedges RING the stick with the L3/R3 click zone as the
// center hub — adjacent, never covering it.

// [fn, identity label, identity palette, cx, cy, w, h, kind]
type ZoneDef = [string, string, string, number, number, number, number, string];

const ZONE_XBOX: ZoneDef[] = [
  ["lt", "LT", "sh", 31.0, 4.6, 10.0, 5.2, "trigger"],
  ["lb", "LB", "sh", 34.0, 10.9, 11.0, 5.2, "bumper"],
  ["rb", "RB", "sh", 66.0, 10.9, 11.0, 5.2, "bumper"],
  ["rt", "RT", "sh", 69.0, 4.6, 10.0, 5.2, "trigger"],
  ["Y", "Y", "xy", 75.2, 31.1, 7.2, 8.4, "round"],
  ["B", "B", "xb", 82.0, 39.6, 7.2, 8.4, "round"],
  ["A", "A", "xa", 75.3, 48.3, 7.2, 8.4, "round"],
  ["X", "X", "xx", 68.7, 39.7, 7.2, 8.4, "round"],
  ["guide", "guide", "txt", 50.0, 27.0, 9.0, 11.0, "round"],
  ["back", "view", "txt", 44.0, 39.0, 6.5, 8.0, "chip"],
  ["start", "menu", "txt", 56.0, 39.0, 6.5, 8.0, "chip"],
  ["lthumb", "L3", "hub", 24.0, 39.7, 8.0, 10.0, "round"],
  ["ly.max", "↑", "dir", 24.0, 31.7, 7.0, 6.0, "chip"],
  ["ly.min", "↓", "dir", 24.0, 47.7, 7.0, 6.0, "chip"],
  ["lx.min", "←", "dir", 17.25, 39.7, 5.5, 7.0, "chip"],
  ["lx.max", "→", "dir", 30.75, 39.7, 5.5, 7.0, "chip"],
  ["dpad.up", "↑", "dir", 36.4, 50.6, 7.0, 9.0, "chip"],
  ["dpad.down", "↓", "dir", 36.4, 69.2, 7.0, 9.0, "chip"],
  ["dpad.left", "←", "dir", 29.2, 59.9, 7.0, 9.0, "chip"],
  ["dpad.right", "→", "dir", 43.6, 59.9, 7.0, 9.0, "chip"],
  ["rthumb", "R3", "hub", 62.5, 58.4, 8.0, 10.0, "round"],
  ["ry.max", "↑", "dir", 62.5, 50.4, 7.0, 6.0, "chip"],
  ["ry.min", "↓", "dir", 62.5, 66.4, 7.0, 6.0, "chip"],
  ["rx.min", "←", "dir", 55.75, 58.4, 5.5, 7.0, "chip"],
  ["rx.max", "→", "dir", 69.25, 58.4, 5.5, 7.0, "chip"],
];

const ZONE_DS4: ZoneDef[] = [
  ["lt", "L2", "sh", 17.0, 4.6, 9.5, 5.2, "trigger"],
  ["lb", "L1", "sh", 19.5, 10.9, 10.5, 5.2, "bumper"],
  ["rb", "R1", "sh", 80.5, 10.9, 10.5, 5.2, "bumper"],
  ["rt", "R2", "sh", 83.0, 4.6, 9.5, 5.2, "trigger"],
  ["Y", "△", "pt", 81.2, 29.2, 7.0, 9.0, "round"],
  ["B", "○", "po", 88.4, 38.8, 7.0, 9.0, "round"],
  ["A", "✕", "pc", 81.3, 48.1, 7.0, 9.0, "round"],
  ["X", "□", "psq", 74.0, 38.7, 7.0, 9.0, "round"],
  ["back", "share", "txt", 30.0, 25.5, 7.0, 9.0, "chip"],
  ["start", "options", "txt", 70.0, 25.5, 7.0, 9.0, "chip"],
  ["guide", "PS", "txt", 50.0, 63.0, 8.0, 10.0, "round"],
  ["lthumb", "L3", "hub", 33.8, 56.8, 8.0, 10.0, "round"],
  ["ly.max", "↑", "dir", 33.8, 48.8, 7.0, 6.0, "chip"],
  ["ly.min", "↓", "dir", 33.8, 64.8, 7.0, 6.0, "chip"],
  ["lx.min", "←", "dir", 27.05, 56.8, 5.5, 7.0, "chip"],
  ["lx.max", "→", "dir", 40.55, 56.8, 5.5, 7.0, "chip"],
  ["dpad.up", "↑", "dir", 18.5, 31.5, 5.4, 7.2, "chip"],
  ["dpad.down", "↓", "dir", 18.5, 46.6, 5.4, 7.2, "chip"],
  ["dpad.left", "←", "dir", 12.9, 39.2, 5.4, 7.2, "chip"],
  ["dpad.right", "→", "dir", 23.9, 39.2, 5.4, 7.2, "chip"],
  ["rthumb", "R3", "hub", 66.1, 56.8, 8.0, 10.0, "round"],
  ["ry.max", "↑", "dir", 66.1, 48.8, 7.0, 6.0, "chip"],
  ["ry.min", "↓", "dir", 66.1, 64.8, 7.0, 6.0, "chip"],
  ["rx.min", "←", "dir", 59.35, 56.8, 5.5, 7.0, "chip"],
  ["rx.max", "→", "dir", 72.85, 56.8, 5.5, 7.0, "chip"],
];

export function isPlaystation(persona: string): boolean {
  return /playstation|ds4|ps4/i.test(persona);
}

// ── The live state store (getter names MUST match MapPage.ts) ──────────────

const [slotLine, setSlotLine] = createSignal("no mappable slots");
const [sourceLine, setSourceLine] = createSignal("not collected");
const [reasonLine, setReasonLine] = createSignal("");
const [cliLine, setCliLine] = createSignal(
  "ksx map --preset <NAME> --function <FUNCTION> --key <KEY>",
);
const [daemonCmd, setDaemonCmd] = createSignal("ksx daemon");
const [backupLine, setBackupLine] = createSignal("Restore backup");
/** v14, the preset surface's identity block: which file, where, and whether a
 *  road home exists. Read straight off the payload — no new verbs. */
const [presetLine, setPresetLine] = createSignal("(no preset)");
const [presetPath, setPresetPath] = createSignal("(unknown)");
const [backupFact, setBackupFact] = createSignal("none yet — the first restore writes one");
/** v9: the selected slot NUMBER as a string — the hidden field every no-JS
 *  form outside the legend list carries (preset actions, the bind-by-name
 *  panel). The server resolves the preset from it. */
const [slotNum, setSlotNum] = createSignal("1");
const [modalPrompt, setModalPrompt] = createSignal("");
const [modalBinding, setModalBinding] = createSignal("");
const [countdownText, setCountdownText] = createSignal("");
const [barStyle, setBarStyle] = createSignal("width:100%");
const [conflictLine, setConflictLine] = createSignal("");
/** The SERVER-RENDERED flash line (no-JS path). The client writes toasts
 *  instead, so these three have no setters here on purpose — see the toast
 *  stack below. */
const [savedLine] = createSignal("");
const [savedAt, setSavedAt] = createSignal("");
const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");
/** v7 multi-select: the header toggle's look/label, and the floating bar's
 *  count line. Class strings, not shows (ledger #13) — the toggle button is
 *  always in the DOM, hidden until map.ts marks the island `.js`. */
const SEL_TOGGLE_OFF = "btn btn-row seltoggle";
const SEL_TOGGLE_LABEL_OFF = "Select multiple";
const [selToggleCls, setSelToggleCls] = createSignal(SEL_TOGGLE_OFF);
const [selToggleLabel, setSelToggleLabel] = createSignal(SEL_TOGGLE_LABEL_OFF);
const [selCountLine, setSelCountLine] = createSignal("");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [pillPaused, setPillPaused] = createSignal(false);
const [noDaemon, setNoDaemon] = createSignal(false);
const [sessionRunning, setSessionRunning] = createSignal(false);
const [pausedBar, setPausedBar] = createSignal(false);
const [readOnly, setReadOnly] = createSignal(false);
const [canLearn, setCanLearn] = createSignal(false);
const [artXbox, setArtXbox] = createSignal(false);
const [artDs4, setArtDs4] = createSignal(false);
const [hasBackup, setHasBackup] = createSignal(false);
const [savedOk] = createSignal(false);
const [savedErr] = createSignal(false);
const [modalOpen, setModalOpen] = createSignal(false);
const [modalListening, setModalListening] = createSignal(false);
const [modalBound, setModalBound] = createSignal(false);
const [modalConflict, setModalConflict] = createSignal(false);
const [selBar, setSelBar] = createSignal(false);

const [slotTabs, setSlotTabs] = createSignal<SlotTab[]>([]);
const [zones, setZones] = createSignal<ZoneRow[]>([]);
const [legendRows, setLegendRows] = createSignal<LegendRow[]>([]);
/** The toast stack, newest FIRST. Client-only: SSR paints an empty list (the
 *  `<!--f:lN-->` markers are still emitted, which is what lets the adoption
 *  path insert into it later), so no-JS users keep the server-rendered flash
 *  line below the preset card and nothing else changes. */
const [toasts, setToasts] = createSignal<ToastRow[]>([]);
/** The preset-actions card's class: "card pactions" when the daemon can
 *  restore, "card pactions off" (inert look, clicks flash the reason) when
 *  not. A class string, not a show — the card never unmounts. */
const [actionsCls, setActionsCls] = createSignal("card pactions off");

// ── v11: the macro editor's own signals (twins in MapPage.ts) ──────────────
// v12 defaults say "nothing is loaded", never a made-up macro name: the old
// "my-macro" placeholder existed only in the browser, so binding a trigger to
// it came back "preset defines no macro called my-macro". A name on this card
// is now always a name the PRESET holds.
const [macroHead, setMacroHead] = createSignal("no macro loaded yet");
const [macroRuleLine, setMacroRuleLine] = createSignal("");
/** v16: the ring, stated once under the grid — what the eight columns of a
 *  direction group are, and exactly what ticking a diagonal writes. */
const [macroRingLine, setMacroRingLine] = createSignal("");
const [macroPolicyLine, setMacroPolicyLine] = createSignal(
  "on release: finish · retrigger: ignore · interrupt: none",
);
const [macroNote, setMacroNote] = createSignal("");
const [macroTriggerLine, setMacroTriggerLine] = createSignal(
  "no trigger key yet — nothing starts this macro",
);
const [macroFnName, setMacroFnName] = createSignal("");
const [macroName, setMacroName] = createSignal("");
const [macroCliLine, setMacroCliLine] = createSignal(
  "ksx map --preset <NAME> --function macro.<NAME> --key <KEY>",
);
const [macroToml, setMacroToml] = createSignal("");
const [macroCardCls, setMacroCardCls] = createSignal("card macrocard off");
const [macroGridCls, setMacroGridCls] = createSignal("macgrid empty");
const [macroDirtyLine, setMacroDirtyLine] = createSignal("");
/** The DETAIL line under the grid: which step the frame maths and the
 *  allow-short box are about. FIX 2 took the duration editor out of this panel
 *  and put it on every row, so this line no longer instructs anybody to select
 *  anything — it reports what is focused, and says where a time is edited. */
const [macroStepLine, setMacroStepLine] = createSignal(
  "every step's time is its own box on its own row — type in the row you want",
);
/** v12: the Save button's own look — "btn macsave" when there is nothing to
 *  write, "… dirty" the moment the draft differs from the file. A class
 *  string, never a show (ledger #13/#14). */
const [macroSaveCls, setMacroSaveCls] = createSignal("btn btn-mini macsave off");
/** v15/FIX 2: Save's inline question about short steps, and the class that
 *  shows the bar holding it. Two scalars and no show, like everything else on
 *  this card (ledger #13/#14). Empty + "off" is the resting state: a macro with
 *  nothing below the sampling floor never sees this. */
const [macroConfirmCls, setMacroConfirmCls] = createSignal("macconfirm off");
const [macroConfirmLine, setMacroConfirmLine] = createSignal("");
/** v15/FIX 1c: which mechanism the "common motions" buttons will write, and
 *  why — read off THIS SLOT's own bound direction keys. */
const [macroMotionLine, setMacroMotionLine] = createSignal("");
/** v14: the per-macro ON/OFF switch. Two scalars, no show (ledger #13/#14):
 *  a class string for the look and a label for the word on the button.
 *
 *  This one IS a button, unlike the slot switch below, because it writes a
 *  preset — the same `map-macro` verb every other macro write uses, in its
 *  TOGGLE spelling (no `steps`, so the table on disk keeps everything and only
 *  the flag moves). */
const [macroEnableCls, setMacroEnableCls] = createSignal("btn btn-mini macen off");
const [macroEnableLabel, setMacroEnableLabel] = createSignal("Enabled");
/** v14: the slot-wide `macros = "off"` switch, in words. Blank when the slot
 *  runs macros, which is every slot until somebody says otherwise.
 *
 *  Deliberately NOT a button. It lives in config.toml (or the games.toml
 *  profile), and Studio has no config writer at all — every write on this page
 *  goes through a preset verb. A switch that silently did nothing would be
 *  worse than a sentence that says exactly which line to change, so this is
 *  the sentence. The macro card renders it above the grid, because a card full
 *  of steps that cannot run has to say so before it shows them. */
const [slotMacrosLine, setSlotMacrosLine] = createSignal("");
/** v12: the frame arithmetic, live, wherever a duration is edited (Victor: "a
 *  60fps frame is only like sixteenth milliseconds? maybe we can show that
 *  math"). Carries the sampling floor in the SAME units, so "too short" needs
 *  no other explanation. */
const [macroMathLine, setMacroMathLine] = createSignal("");
/** v13: the REPEAT policy's own live math — the answer to "where is the option
 *  to turn autorepeat on?" and, once it is on, to "why is my 30 Hz turbo not
 *  30 Hz?". Same treatment as the duration line above: both numbers, always,
 *  never a silent substitution. */
const [macroTurboLine, setMacroTurboLine] = createSignal("");
/** The rate box's value, in whichever unit the file authored — `turbo_hz` or
 *  `gap_ms`. Blank when the macro carries no rate at all. */
const [macroTurboValue, setMacroTurboValue] = createSignal("");
/** The learn modal's auto-fire line: what this control does today, and what a
 *  rate typed into the box beside it would really deliver. */
const [modalTurboLine, setModalTurboLine] = createSignal("");
/** The trigger block's class: inert while the preset holds no macro, because
 *  a key that starts nothing is exactly the confusion this card had. */
const [macroTrigCls, setMacroTrigCls] = createSignal("mactrigger off");
const [macroTabs, setMacroTabs] = createSignal<MacroTab[]>([]);
const [macroCols, setMacroCols] = createSignal<MacroCol[]>([]);
const [macroGroups, setMacroGroups] = createSignal<MacroGroup[]>([]);
const [macroRows, setMacroRows] = createSignal<MacroRow[]>([]);
const [macroCells, setMacroCells] = createSignal<MacroCell[]>([]);

// ── Client-side selection state (map.ts drives it) ─────────────────────────

let lastPayload: MapPayload | null = null;
let selectedSlot = 0; // slot NUMBER
let selectedFn: string | null = null;
/** The shared hover signal: hovering a zone highlights its legend row and
 *  vice versa (both re-derive with the hot class). Client-only — the server
 *  never emits a hot class (SSR has no hover). */
let hotFn: string | null = null;
/** Mirrors render_map.rs `learnable`: can a click actually record right now?
 *  Drives the z-dead / l-dead look and the ✕ accelerator. */
let liveMapping = false;
/** Mirrors render_map.rs `writable`: can a binding be WRITTEN right now? A
 *  wider condition than [`liveMapping`] on purpose — learning needs the
 *  panel's keys to reach the daemon's listener, writing needs only a daemon
 *  (a running session takes a binding change hot, and a daemon that predates
 *  the learn verbs still has `map`). This is what gates the no-JS forms,
 *  which pick a key instead of listening for one. */
let canWrite = false;

// ── v7 multi-select (FEATURE 2) ────────────────────────────────────────────
// Victor's file-explorer analogy: Ctrl/Shift-click ADDS a control to a
// selection, and one action then applies to all of them. Client-only state —
// nothing here exists without JS, and the no-JS page keeps the v6
// single-click-to-learn behaviour untouched.

/** Selected function names. Iteration order is insertion order; the UI shows
 *  them in TABLE order so the prompt reads like the pad, not like the clicks. */
const selection = new Set<string>();
/** Touch mode: while on, a plain tap toggles selection instead of learning.
 *  The discoverable half of the feature (Victor: "tick something"). */
let multiMode = false;

/** Repaint both readers from the current slot — every selection/hover change
 *  goes through here, so the art and the legend can never disagree. */
function refreshRows(): void {
  const slot = currentSlot();
  if (!slot) return;
  setZones(zoneRows(slot));
  setLegendRows(legendRowsFor(slot));
}

export function setHot(fn: string | null): void {
  if (hotFn === fn) return;
  hotFn = fn;
  refreshRows();
}

/** Selected controls in the order they were PICKED — a Set keeps insertion
 *  order, and the prompt reading back "A, B, RT" in the order the user tapped
 *  them is how they check the selection before pressing a key. (The legend's
 *  shared-key badges use table order instead: those come from disk and have no
 *  click history.) */
export function selectedFns(): string[] {
  return Array.from(selection);
}

export function selectionCount(): number {
  return selection.size;
}

export function isMultiMode(): boolean {
  return multiMode;
}

/** "A", "✕", "D-pad ▲" — how this persona names a control, for prompts.
 *  A `macro.<name>` function is not on the pad at all; it is named as what it
 *  is, so a toast about a trigger never reads like a button rebind. */
export function identityLabel(fn: string): string {
  if (fn.startsWith("macro.")) return `the “${fn.slice("macro.".length)}” macro trigger`;
  const def = zoneTable().find((z) => z[0] === fn);
  return def ? legendLabel(def[0], def[1]) : fn;
}

export function toggleSelected(fn: string): void {
  if (selection.has(fn)) selection.delete(fn);
  else selection.add(fn);
  syncSelection();
}

export function clearSelection(): void {
  if (selection.size === 0 && !multiMode) return;
  selection.clear();
  syncSelection();
}

export function setMultiMode(on: boolean): void {
  multiMode = on;
  if (!on) selection.clear();
  syncSelection();
}

/** One place where selection state reaches the screen. */
function syncSelection(): void {
  const n = selection.size;
  setSelBar(n > 0);
  setSelCountLine(
    n === 1 ? "1 control selected" : `${n} controls selected`,
  );
  setSelToggleCls(multiMode ? `${SEL_TOGGLE_OFF} on` : SEL_TOGGLE_OFF);
  setSelToggleLabel(multiMode ? "Selecting — tap controls" : SEL_TOGGLE_LABEL_OFF);
  refreshRows();
}

export function currentSlot(): MapperSlot | null {
  if (!lastPayload) return null;
  return (
    lastPayload.mapper.slots.find((s) => s.number === selectedSlot) ??
    lastPayload.mapper.slots[0] ??
    null
  );
}

export function selectSlot(num: number): void {
  selectedSlot = num;
  // A selection belongs to ONE preset — carrying it across slots would apply
  // an action to controls the user is no longer looking at. So does a macro
  // draft: it is a sequence over THIS pad's controls.
  selection.clear();
  setSelBar(false);
  resetMacroDraft();
  if (lastPayload) applyMap(lastPayload);
}

export function selectFn(fn: string | null): void {
  selectedFn = fn;
  refreshCliLine();
}

export function selectedFnName(): string | null {
  return selectedFn;
}

export function learnAllowed(): boolean {
  return canLearn();
}

function refreshCliLine(): void {
  const slot = currentSlot();
  const fnPart = selectedFn ?? "<FUNCTION>";
  setCliLine(
    slot
      ? `ksx map --preset "${slot.preset}" --function ${fnPart} --key <KEY>`
      : "ksx map --preset <NAME> --function <FUNCTION> --key <KEY>",
  );
}

// ── Derivations (mirror render_map.rs; pinned there by unit tests) ─────────

/** Every key bound to `fn`, file order — the unit the mapper works in.
 *  MANY KEYS → ONE CONTROL is native to the engine and to the TOML
 *  (`A = ["S", "Enter"]`, press either; docs/INPUT-TRANSFORMS.md §1a). */
export function keysOf(slot: MapperSlot, fn: string): string[] {
  return slot.bindings[fn] ?? [];
}

/** The separator between a control's keys. A MIDDOT, never `+`: `S+Enter`
 *  reads as the chord it is not — these are alternatives. */
const KEY_SEP = " · ";

/** "G", "S · Enter", or "—" — every key, for tooltips and prompts. */
function keyTag(slot: MapperSlot, fn: string): string {
  const keys = keysOf(slot, fn);
  return keys.length > 0 ? keys.join(KEY_SEP) : "—";
}

/** The ON-ART tag: the first key plus `+N` for the ones that do not fit. */
function zoneTag(keys: string[]): string {
  if (keys.length === 0) return "";
  return keys.length === 1 ? keys[0] : `${keys[0]} +${keys.length - 1}`;
}

/** How many key chips a legend row draws before summarizing the tail —
 *  mirrors render_map.rs KEY_CHIPS. */
const KEY_CHIPS = 3;

/** " (2 keys — any one of them presses it)", or "". Two key tags side by side
 *  read just as easily as "both at once", which is chord semantics and wrong;
 *  this says which one it is. */
function eitherNote(count: number): string {
  return count > 1 ? ` (${count} keys — any one of them presses it)` : "";
}

/** The zone table of the slot on screen. */
function zoneTable(): ZoneDef[] {
  const slot = currentSlot();
  return slot && isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
}

/** How many co-bound controls a shared-key badge names before summarizing. */
const SHARE_MAX = 3;

/** Mirrors render_map.rs `shared_labels`: per zone (table order), the LABELS
 *  of the other controls this preset binds to the same key. A key bound twice
 *  is a multi-bind, not a conflict (docs/INPUT-TRANSFORMS.md §1a) — this is
 *  the data that lets both readers say so. */
function sharedLabels(slot: MapperSlot): string[][] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const keys = table.map(([fn]) => keysOf(slot, fn));
  // v10: two controls share when their key SETS INTERSECT. One key in common
  // is one key that drives both, whether or not either control has others —
  // comparing the joined tags (as this did) stopped noticing the moment a
  // control held more than one key.
  return keys.map((mine, i) =>
    mine.length === 0
      ? []
      : table
          .filter((_, j) => j !== i && keys[j].some((k) => mine.includes(k)))
          .map(([fn, label]) => legendLabel(fn, label)),
  );
}

/** "also A · B", capped — mirrors render_map.rs `share_text`. */
function shareText(names: string[]): string {
  if (names.length === 0) return "";
  const text = `also ${names.slice(0, SHARE_MAX).join(" · ")}`;
  return names.length > SHARE_MAX ? `${text} +${names.length - SHARE_MAX}` : text;
}

function shareTitle(key: string, names: string[]): string {
  return names.length === 0 ? "" : `${key} also drives ${names.join(", ")}`;
}

function zoneRows(slot: MapperSlot): ZoneRow[] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const dead = liveMapping ? "" : " z-dead";
  const shared = sharedLabels(slot);
  return table.map(([fn, label, idk, cx, cy, w, h, kind], i) => {
    const keys = keysOf(slot, fn);
    const key = keyTag(slot, fn);
    const share = shared[i];
    // z-unbound hides the tag pill via CSS: `:empty` cannot work, the SSR
    // text slot leaves marker nodes inside the span. z-dead is the VISIBLY
    // disabled look — never the `disabled` attribute, which would swallow the
    // click that has to answer "why can't I map right now?".
    return {
      fn,
      cls:
        `zone z-${kind}${key === "—" ? " z-unbound" : ""}${dead}` +
        `${share.length > 0 ? " z-shared" : ""}${fn === hotFn ? " z-hot" : ""}` +
        `${selection.has(fn) ? " z-sel" : ""}`,
      // FEATURE 1: the identity, drawn on art that has no letters of its own.
      id: label,
      idcls: `zid id-${idk}`,
      style:
        `left:${(cx - w / 2).toFixed(1)}%;top:${(cy - h / 2).toFixed(1)}%;` +
        `width:${w.toFixed(1)}%;height:${h.toFixed(1)}%`,
      title:
        `${fn} — ${key}${eitherNote(keys.length)}` +
        (share.length > 0 ? ` (${shareTitle(key, share)})` : ""),
      // The art shows the first key and counts the rest; the title above and
      // the legend below name every one.
      tag: zoneTag(keys),
    };
  });
}

/** "LS ", "RS ", "D-pad ", "" — the prefix that keeps four identical arrow
 *  glyphs apart in a flat list. */
function legendGroup(fn: string): string {
  if (fn.startsWith("lx.") || fn.startsWith("ly.")) return "LS ";
  if (fn.startsWith("rx.") || fn.startsWith("ry.")) return "RS ";
  if (fn.startsWith("dpad.")) return "D-pad ";
  return "";
}

/** "LS ▲", "D-pad ◀", "✕" — group + identity, for tooltips and prompts. */
function legendLabel(fn: string, label: string): string {
  return `${legendGroup(fn)}${label}`;
}

/** The control's authored auto-fire rate, or null. */
export function turboHzOf(slot: MapperSlot, fn: string): number | null {
  const hz = slot.turbo?.[fn];
  return typeof hz === "number" ? hz : null;
}

/** Mirror of `ksx_core::TurboBinding` — the arithmetic the ENGINE runs, so the
 *  badge and the pad cannot disagree. Pinned against the Rust side in
 *  render_map.rs. */
const TURBO_MAX_HZ = 30;

function turboOnMs(hz: number): number {
  const clamped = Math.min(Math.max(hz, 1), TURBO_MAX_HZ);
  return Math.max(Math.floor((Math.floor(1000 / clamped) + 1) / 2), MIN_STEP_MS);
}

function turboOffMs(hz: number): number {
  const clamped = Math.min(Math.max(hz, 1), TURBO_MAX_HZ);
  return Math.max(Math.floor(1000 / clamped) - turboOnMs(hz), MIN_STEP_MS);
}

export function effectiveTurboHz(hz: number): number {
  const cycle = turboOnMs(hz) + turboOffMs(hz);
  return Math.floor((1000 + Math.floor(cycle / 2)) / cycle);
}

function turboTag(slot: MapperSlot, fn: string): string {
  const hz = turboHzOf(slot, fn);
  if (hz === null) return "";
  const effective = effectiveTurboHz(hz);
  return effective === hz ? `turbo ${hz} Hz` : `turbo ~${effective} Hz`;
}

function turboTitle(slot: MapperSlot, fn: string): string {
  const hz = turboHzOf(slot, fn);
  if (hz === null) {
    return (
      `${fn} does not auto-fire — hold its key and it stays down. "Turbo" in the learn ` +
      "dialog (or the box in this row without JavaScript) gives it a rate."
    );
  }
  const effective = effectiveTurboHz(hz);
  let line =
    `${fn} AUTO-FIRES while any of its keys is held: ${turboOnMs(hz)} ms pressed, ` +
    `${turboOffMs(hz)} ms released, one clock however many keys point at it.`;
  if (effective !== hz) {
    line +=
      ` The file asks for ${hz} Hz and gets about ${effective} Hz: a press AND a release ` +
      `must each survive a 60 Hz poll (${MIN_STEP_MS} ms), so ~15 Hz is the fastest ` +
      "anything can be delivered.";
  }
  return line;
}

function legendRowsFor(slot: MapperSlot): LegendRow[] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const shared = sharedLabels(slot);
  return table.map(([fn, label, idk], i) => {
    const keys = keysOf(slot, fn);
    const key = keyTag(slot, fn);
    const unbound = key === "—";
    const share = shared[i];
    const chip = (n: number): string => keys[n] ?? "";
    // `lk1` right-aligns the group: only the first chip may take the row's
    // free space (studio.css).
    const chipCls = (n: number): string =>
      `lkc${n === 0 ? " lk1" : ""}${keys[n] === undefined ? " off" : ""}`;
    // The ✕ is a SIBLING of the key tag, never the tag itself: clicking a key
    // must not be what deletes it.
    const chipX = (n: number): string =>
      keys[n] !== undefined && liveMapping ? "lkx" : "lkx off";
    const chipRm = (n: number): string => (keys[n] === undefined ? "" : `${fn}|${keys[n]}`);
    const chipTitle = (n: number): string => {
      const k = keys[n];
      if (k === undefined) return "";
      const rest = keys.filter((other) => other !== k);
      return rest.length > 0
        ? `remove ${k} from ${fn} — it keeps ${rest.join(KEY_SEP)}`
        : `remove ${k} from ${fn} — it is the only key`;
    };
    const extra = Math.max(0, keys.length - KEY_CHIPS);
    return {
      fn,
      label: legendLabel(fn, label),
      id: label,
      idcls: `lid id-${idk}`,
      group: legendGroup(fn),
      key,
      cls:
        `lrow${unbound ? " l-unbound" : ""}${liveMapping ? "" : " l-dead"}` +
        `${share.length > 0 ? " l-shared" : ""}${keys.length > 1 ? " l-multi" : ""}` +
        `${fn === hotFn ? " l-hot" : ""}${selection.has(fn) ? " l-sel" : ""}`,
      title: `${fn} — ${key}${eitherNote(keys.length)}`,
      share: shareText(share),
      sharetitle: shareTitle(key, share),
      // The desktop accelerator. Only where clearing would do something; the
      // learn modal's "Clear binding" is the primary, touch-first path.
      clear: liveMapping && !unbound ? "✕" : "",
      cleartitle:
        keys.length > 1 ? `clear ${fn} (all ${keys.length} keys)` : `clear ${fn}`,
      k1: chip(0),
      k1cls: chipCls(0),
      k1xcls: chipX(0),
      k1rm: chipRm(0),
      k1title: chipTitle(0),
      k2: chip(1),
      k2cls: chipCls(1),
      k2xcls: chipX(1),
      k2rm: chipRm(1),
      k2title: chipTitle(1),
      k3: chip(2),
      k3cls: chipCls(2),
      k3xcls: chipX(2),
      k3rm: chipRm(2),
      k3title: chipTitle(2),
      kmore: extra > 0 ? `+${extra}` : "",
      kmorecls: extra > 0 ? "lkmore" : "lkmore off",
      kmoretitle: extra > 0 ? `${extra} more key(s): ${keys.join(KEY_SEP)}` : "",
      addtitle: `add the picked key to ${fn} — it keeps ${unbound ? "nothing yet" : key}`,
      rmtitle: `remove just the picked key from ${fn} (${key})`,
      // v9's no-JS form fields. `bindcls` is a class string, never a show
      // (ledger #13/#15) — the form is always there, dimmed when nothing can
      // be written, because a POST that the daemon refuses still comes back
      // as a flash sentence, which beats a control that is not there.
      slot: String(slot.number),
      bindcls: canWrite ? "lbind nojs" : "lbind nojs off",
      bindtitle: `bind ${fn} (${legendLabel(fn, label)})`,
      turbo: turboTag(slot, fn),
      turbotitle: turboTitle(slot, fn),
      turboval: turboHzOf(slot, fn) === null ? "" : String(turboHzOf(slot, fn)),
    };
  });
}

function learnable(p: MapPayload): boolean {
  return p.session.reachable && !p.session.running && p.learn.state !== "unavailable";
}

function reason(p: MapPayload): string {
  if (p.mapper.slots.length === 0) return `nothing to map — ${p.mapper.source}`;
  if (!p.session.reachable)
    return (
      "read-only: no daemon control channel — start the daemon (tray, or `ksx daemon`), " +
      "or bind from a shell with the command below"
    );
  if (p.session.running)
    return (
      // WORD FOR WORD what render_map.rs's `reason_line` renders — this is the
      // one banner whose text differs between the two branches, so a drift
      // here is a VISIBLE flash: the server paints one sentence and the client
      // replaces it a few milliseconds later. It said "the Pause button" until
      // 2026-08-06, which also named a control that does not exist; the button
      // is labelled "Pause emulation & map". Pinned by the SSR/hydration
      // parity suite (pwtest/ssr-hydration-parity.test.mjs).
      "read-only while emulation runs: the panel's keys are captured, so ksx cannot " +
      'hear them for mapping. Use "Pause emulation & map" above, or bind from a ' +
      "shell with the command below"
    );
  if (p.learn.state === "unavailable")
    return (
      "read-only: the daemon does not answer the learn verbs " +
      `(${p.learn.error ?? "no reason reported"}) — restart it on the current ksx build, ` +
      "or bind from a shell with the command below"
    );
  return "";
}

/** Write one /api/map payload into every signal. Keeps the client's own slot
 *  selection; modal/flash state is owned by map.ts. Safe before adoption AND
 *  per poll. */
export function applyMap(p: MapPayload): void {
  lastPayload = p;
  if (!p.mapper.slots.some((s) => s.number === selectedSlot)) {
    selectedSlot = p.selected;
  }
  const slot = currentSlot();
  // Derived BEFORE the row builders run — they read it for the dead look.
  liveMapping = learnable(p) && slot !== null;
  canWrite = p.session.reachable && slot !== null;
  // The daemon is answering and running again: whatever we paused has been
  // started back up, so drop the paused affordance.
  if (p.session.reachable && p.session.running) {
    paused = false;
    pausedProfile = null;
  }

  setSlotTabs(
    p.mapper.slots.map((s) => ({
      num: String(s.number),
      label: `P${s.number} · ${s.preset}`,
      cls: slot && s.number === slot.number ? "tab active" : "tab",
      href: `/map?slot=${s.number}`,
      player: `P${s.number}`,
      preset: s.preset,
      pad: s.persona_label,
      kbd: s.keyboard,
      rowcls: slot && s.number === slot.number ? "strow on" : "strow",
    })),
  );
  setSlotNum(String(slot ? slot.number : p.selected));
  setZones(slot ? zoneRows(slot) : []);
  setLegendRows(slot ? legendRowsFor(slot) : []);
  setSlotLine(
    slot ? `P${slot.number} · ${slot.persona_label} · ${slot.preset}` : "no mappable slots",
  );
  setSourceLine(`${p.mapper.source} — config root: ${p.mapper.config_root}`);
  setPresetLine(slot ? slot.preset : "(no preset)");
  setPresetPath(
    slot ? `${p.mapper.config_root}\\presets\\${slot.preset}.toml` : p.mapper.config_root,
  );
  setBackupFact(
    slot && slot.backup
      ? `newest ${slot.backup}`
      : "none yet — the first restore writes one",
  );
  setGeneratedAt(p.mapper.generated_at);

  const live = liveMapping;
  setReasonLine(reason(p));
  setReadOnly(!live);
  setCanLearn(live);
  setActionsCls(p.session.reachable ? "card pactions" : "card pactions off");
  setArtXbox(slot !== null && !isPlaystation(slot.persona));
  setArtDs4(slot !== null && isPlaystation(slot.persona));
  setDaemonCmd(
    p.session.profile ? `ksx daemon --game "${p.session.profile}"` : "ksx daemon",
  );
  setHasBackup(slot !== null && slot.backup !== null);
  setBackupLine(slot?.backup ? `Restore backup from ${slot.backup}` : "Restore backup");

  // v11: seed the macro draft from the file, but never over an edit in
  // flight — a 2 s poll that ate a half-painted sequence would be the exact
  // silent data loss this page bans (and with an explicit Save, an unsaved
  // draft is the normal state while authoring, not an edge case). The TRIGGER
  // is re-read either way: that half is written by the `map` verb, so a draft
  // has no business remembering a stale copy of it.
  // v12.1: "untouched" now includes "nobody's hands are on it". A clean draft
  // whose duration box has the caret is still an edit in flight — re-seeding
  // it repaints the very control being typed into.
  if (macroDraft === null || !macroEditorBusy()) {
    seedMacro(null);
  } else {
    const fresh = p.macros.macros.find(
      (m) => m.name.toLowerCase() === macroDraft?.name.toLowerCase(),
    );
    if (fresh && macroDraft) macroDraft.triggers = [...fresh.triggers];
    refreshMacro();
  }

  const running = p.session.reachable && p.session.running;
  const idle = p.session.reachable && !p.session.running;
  setPillRunning(running);
  setPillIdle(idle && !paused);
  setPillDown(!p.session.reachable);
  setPillPaused(idle && paused);
  setNoDaemon(!p.session.reachable);
  setSessionRunning(running);
  setPausedBar(idle && paused);

  refreshCliLine();
}

// ── FIX 0: pause for mapping, and the road back ────────────────────────────
// The daemon refuses to learn while emulation runs, for reasons written out in
// full in daemon/pipe.rs (the capture thread is not a place features get
// added, and a key pressed to be learned would also fire its binding). The
// answer is not to weaken the refusal but to make obeying it ONE CLICK: pause,
// map, resume — with the paused state visible the whole time so nobody walks
// away from a cabinet they stopped.

/** Client-only: this PAGE paused emulation. To the daemon it is just idle. */
let paused = false;
/** The profile that was running when we paused, so Resume restores it. */
let pausedProfile: string | null = null;

/** The pause landed. Flip the affordances NOW rather than re-deriving from
 *  `lastPayload` — that payload still says "running" (it predates the stop by
 *  definition), and applyMap's own rule "running ⇒ not paused" would undo the
 *  pause the instant it was set. The next poll re-derives everything anyway. */
export function markPaused(profile: string | null): void {
  paused = true;
  pausedProfile = profile;
  setPillRunning(false);
  setPillIdle(false);
  setPillPaused(true);
  setSessionRunning(false);
  setPausedBar(true);
}

export function clearPaused(): void {
  paused = false;
  pausedProfile = null;
  setPillPaused(false);
  setPausedBar(false);
}

export function profileToResume(): string | null {
  return pausedProfile;
}

export function isPaused(): boolean {
  return paused;
}

/** The profile running right now — remembered at pause time. */
export function liveProfile(): string | null {
  return lastPayload?.session.profile ?? null;
}

/** The preset the visible slot maps — every preset-level verb's argument. */
export function currentPreset(): string | null {
  return currentSlot()?.preset ?? null;
}

/** The key(s) bound to `fn` right now, or null when unbound. Feeds the learn
 *  modal's "currently …" line and its Clear button. */
export function currentBinding(fn: string): string | null {
  const slot = currentSlot();
  if (!slot) return null;
  const keys = fn.startsWith("macro.") ? macroTriggersOf(fn) : keysOf(slot, fn);
  return keys.length > 0 ? keys.join(KEY_SEP) : null;
}

/** The raw key list bound to `fn` right now — the set every edit is computed
 *  against (add = ∪ {k}, per-key ✕ = ∖ {k}) and what an UNDO has to put back.
 *  Kept separate from [`currentBinding`], which joins it for display. */
export function previousKeys(fn: string): string[] {
  // A macro TRIGGER lives in the preset's `[macros]` triggers, not in the
  // bindings map — so an undo of a trigger write has to read it from there, or
  // it would offer to put back "nothing" over a key that was really set.
  if (fn.startsWith("macro.")) return macroTriggersOf(fn);
  return currentSlot()?.bindings[fn]?.slice() ?? [];
}

/** Can this exact key list be written back through `/api/bind/keys`?
 *
 *  Mirrors ksx-studio's `ControlSource::bind_keys`: the daemon's `map` verb
 *  takes ONE key and replaces the control, so a set of none (a clear) and a
 *  set of one are expressible and a set of two or more is not — the server
 *  refuses it in words rather than writing the first key and dropping the
 *  rest.
 *
 *  This is the single rule behind every Undo offer on the page. It is why an
 *  add onto an unbound control undoes cleanly (back to nothing), why removing
 *  a control's only key undoes cleanly (back to that key), and why undoing a
 *  removal that would restore TWO keys is not offered — offering it would be
 *  a button that silently puts back half the binding. The moment a daemon can
 *  write a key list, this returns true for everything and every one of those
 *  paths becomes undoable with no other change. */
export function writableKeys(keys: string[]): boolean {
  return keys.length <= 1;
}

/** "S · Enter" — a key list as this page says it out loud. */
export function keyList(keys: string[]): string {
  return keys.join(KEY_SEP);
}

/** Why a click cannot record right now — one clause, worst problem first.
 *  `null` means it can. This is what turns a dead click into a sentence. */
export function blockedReason(): string | null {
  const p = lastPayload;
  if (!p) return "no snapshot yet";
  if (p.mapper.slots.length === 0) return "there is nothing to map";
  if (!p.session.reachable) return "no daemon running";
  if (p.session.running) return "emulation is running";
  if (p.learn.state === "unavailable") return "this daemon has no learner";
  return null;
}

/** The studio server itself stopped answering: keep the page, say so. */
export function applyMapUnreachable(): void {
  setReasonLine("ksx-studio not responding — retrying every 2 s");
  setReadOnly(true);
  setCanLearn(false);
  liveMapping = false;
  canWrite = false;
  setActionsCls("card pactions off");
  setPillRunning(false);
  setPillIdle(false);
  setPillPaused(false);
  setPillDown(true);
  setNoDaemon(true);
  setSessionRunning(false);
  setPausedBar(false);
}

/** "Saved 14:32:07" — auto-save made visible (Victor: "where is save?"). */
export function markSaved(): void {
  const now = new Date();
  const two = (n: number) => String(n).padStart(2, "0");
  setSavedAt(
    `Saved ${two(now.getHours())}:${two(now.getMinutes())}:${two(now.getSeconds())}`,
  );
}

// ── Modal + flash state (driven by map.ts) ─────────────────────────────────

export function showListening(
  promptText: string,
  bindingText: string | null,
  remainingMs: number,
  totalMs: number,
): void {
  setModalPrompt(promptText);
  // MAME's UI Clear lives inside the capture prompt; so does ours. Showing the
  // current binding next to it is what makes "Clear binding" obviously safe.
  showLearnMode(false, bindingText);
  setModalBound(bindingText !== null);
  setModalOpen(true);
  setModalListening(true);
  setModalConflict(false);
  updateCountdown(remainingMs, totalMs);
}

/** Echo what the NEXT press will do to a control that already has keys —
 *  replace them, or join them. The armed choice lives in the same line as the
 *  current binding, so the modal never has a hidden mode: the buttons pick,
 *  this sentence confirms. (Unbound controls have no choice to make, so they
 *  get no line at all.) */
export function showLearnMode(add: boolean, bindingText: string | null): void {
  setModalBinding(
    bindingText === null
      ? ""
      : add
        ? `currently ${bindingText} — the next key is ADDED to it (either will press this control)`
        : `currently ${bindingText} — the next key REPLACES it`,
  );
}

/** The modal's AUTO-FIRE line for one control: what it does today, said in the
 *  rate the game will really see. map.ts calls it when the modal opens, and
 *  again after a Set/No-turbo write lands, so the sentence is never a claim
 *  about a file that has since changed. */
export function showLearnTurbo(fn: string | null): void {
  const slot = currentSlot();
  if (slot === null || fn === null) {
    setModalTurboLine("");
    return;
  }
  const hz = turboHzOf(slot, fn);
  if (hz === null) {
    setModalTurboLine(
      "This control does not auto-fire: hold its key and it stays down. Type a number of " +
        "presses a second and press \u201cSet turbo\u201d to make it fire while the key is held \u2014 " +
        "one clock for the control, however many keys point at it. 10\u201312 Hz is the usual " +
        "cabinet setting; above ~15 Hz nothing more gets through, because a press AND a " +
        "release must each survive a 60 Hz poll.",
    );
    return;
  }
  const effective = effectiveTurboHz(hz);
  setModalTurboLine(
    effective === hz
      ? `This control auto-fires at ${hz} Hz while any of its keys is held. ` +
          "\u201cNo turbo\u201d (or 0) turns it off."
      : `This control asks for ${hz} Hz and actually fires at about ${effective} Hz \u2014 a ` +
          "press AND a release must each survive a 60 Hz poll, so ~15 Hz is the ceiling. " +
          "\u201cNo turbo\u201d (or 0) turns it off.",
  );
}

export function updateCountdown(remainingMs: number, totalMs: number): void {
  const secs = Math.max(0, remainingMs) / 1000;
  setCountdownText(`${secs.toFixed(1)} s`);
  const pct = totalMs > 0 ? Math.max(0, Math.min(100, (remainingMs / totalMs) * 100)) : 0;
  setBarStyle(`width:${pct.toFixed(1)}%`);
}

export function showConflict(promptText: string, line: string): void {
  setModalPrompt(promptText);
  setModalOpen(true);
  setModalListening(false);
  setModalBound(false);
  setModalConflict(true);
  setConflictLine(line);
}

export function closeModal(): void {
  setModalOpen(false);
  setModalListening(false);
  setModalBound(false);
  setModalConflict(false);
}

/** Is the learn modal on screen? The browser-focus guard keys off this. */
export function modalIsOpen(): boolean {
  return modalOpen();
}

// ── The toast stack (v8): optimistic action + a road home ──────────────────
// MAPPER-UX commandment 5 asked for a guaranteed road home; v7 spelled it
// "are you sure?" before four different writes. A confirm dialog is a toll
// paid by every correct action to insure against the rare wrong one, and on a
// cabinet phone it is a modal you dismiss without reading. So: the action
// fires IMMEDIATELY, and the report of what happened carries the way back.
//
// One toast = one plain sentence + (when the action can honestly be reversed)
// an Undo button. Undo is single-level per toast: once it lands the toast
// collapses to what it undid and the button goes. Undo is composed from the
// verbs that already exist — `/api/bind` with the remembered previous key,
// `/api/preset/restore latest-backup` for the whole-preset writes (which
// snapshot a timestamped .bak before writing, so the newest backup IS the
// pre-action state). Nothing new was added to the daemon for this.
//
// The signals below (savedLine/savedOk/savedErr) stay: they are the
// SERVER-RENDERED flash channel for a no-JS page, and the SSR seam still
// carries them (render_map.rs). The client no longer writes them — its
// feedback is the stack.

/** How long a toast lives when nobody touches it. Longer than the old 5 s
 *  flash on purpose: it now carries an ACTION, so it has to outlive the
 *  double-take that follows an unexpected result. */
const TOAST_MS = 8000;
/** Three at a time. The fourth pushes the oldest off the bottom — a stack
 *  taller than this stops being feedback and becomes a wall. */
const TOAST_MAX = 3;

export type ToastKind = "ok" | "warn" | "err";

export interface ToastOptions {
  kind?: ToastKind;
  /** Runs when the user hits Undo (or Ctrl+Z on the newest undoable toast).
   *  Resolves `null` when the undo landed, or the REASON it did not — which
   *  becomes an error toast, never a silent no-op. Absent/null = this action
   *  has no honest undo, so no button is offered. */
  undo?: (() => Promise<string | null>) | null;
  /** The sentence the toast collapses to once the undo lands. */
  undone?: string;
}

interface LiveToast {
  id: string;
  text: string;
  kind: ToastKind;
  undo: (() => Promise<string | null>) | null;
  undone: string;
  /** An undo is in flight: the button hides so it cannot be double-fired. */
  busy: boolean;
  /** Milliseconds left before auto-dismiss; frozen while hovered/focused. */
  remaining: number;
  deadline: number;
  timer?: ReturnType<typeof setTimeout>;
}

let toastSeq = 0;
let liveToasts: LiveToast[] = [];
/** Pointer or focus is inside the stack: nothing may vanish under the hand
 *  that is reaching for its Undo button. */
let toastsHeld = false;

function syncToasts(): void {
  setToasts(
    liveToasts.map((t) => ({
      id: t.id,
      cls: `toast toast-${t.kind}`,
      text: t.text,
      undocls: t.undo !== null && !t.busy ? "btn btn-undo" : "btn btn-undo off",
      undotitle: "Undo this (Ctrl+Z)",
      dismisstitle: "Dismiss",
    })),
  );
}

function armToast(t: LiveToast): void {
  if (toastsHeld) return;
  t.deadline = Date.now() + t.remaining;
  t.timer = setTimeout(() => dismissToast(t.id), t.remaining);
}

function holdToast(t: LiveToast): void {
  if (t.timer !== undefined) {
    clearTimeout(t.timer);
    t.timer = undefined;
  }
  if (t.deadline > 0) t.remaining = Math.max(1200, t.deadline - Date.now());
}

/** Hover/focus pauses every timer in the stack — the stack is one target as
 *  far as a hand is concerned. */
export function holdToasts(): void {
  if (toastsHeld) return;
  toastsHeld = true;
  for (const t of liveToasts) holdToast(t);
}

export function releaseToasts(): void {
  if (!toastsHeld) return;
  toastsHeld = false;
  for (const t of liveToasts) if (!t.busy) armToast(t);
}

/** Report what just happened. Returns the toast id so a caller can replace a
 *  progress line with its own result ([`replaceToast`]). */
export function pushToast(text: string, opts: ToastOptions = {}): string {
  const t: LiveToast = {
    id: `t${++toastSeq}`,
    text,
    kind: opts.kind ?? "ok",
    undo: opts.undo ?? null,
    undone: opts.undone ?? "Undone.",
    busy: false,
    remaining: TOAST_MS,
    deadline: 0,
  };
  liveToasts = [t, ...liveToasts];
  while (liveToasts.length > TOAST_MAX) {
    const gone = liveToasts.pop();
    if (gone?.timer !== undefined) clearTimeout(gone.timer);
  }
  armToast(t);
  syncToasts();
  return t.id;
}

/** "binding 3 controls…" → the real answer, in the same toast. */
export function replaceToast(id: string | null, text: string, opts: ToastOptions = {}): string {
  const t = id === null ? undefined : liveToasts.find((x) => x.id === id);
  if (!t) return pushToast(text, opts);
  holdToast(t);
  t.text = text;
  t.kind = opts.kind ?? "ok";
  t.undo = opts.undo ?? null;
  t.undone = opts.undone ?? "Undone.";
  t.busy = false;
  t.remaining = TOAST_MS;
  armToast(t);
  syncToasts();
  return t.id;
}

export function dismissToast(id: string): void {
  const t = liveToasts.find((x) => x.id === id);
  if (!t) return;
  if (t.timer !== undefined) clearTimeout(t.timer);
  liveToasts = liveToasts.filter((x) => x !== t);
  syncToasts();
}

/** The newest toast that can still be undone — what Ctrl+Z means. */
export function newestUndoable(): string | null {
  return liveToasts.find((t) => t.undo !== null && !t.busy)?.id ?? null;
}

/** Run one toast's undo. Single-level: on success the button disappears and
 *  the toast becomes the record of the reversal. On failure the toast turns
 *  error-styled and NAMES the reason, keeping the button so it can be tried
 *  again — a dead Undo would be exactly the silent no-op this page bans. */
export async function runUndo(id: string | null): Promise<void> {
  if (id === null) return;
  const t = liveToasts.find((x) => x.id === id);
  if (!t || t.undo === null || t.busy) return;
  const original = t.text;
  t.busy = true;
  t.text = "undoing…";
  holdToast(t);
  syncToasts();
  let failure: string | null;
  try {
    failure = await t.undo();
  } catch {
    failure = "the undo request failed — is ksx studio still running?";
  }
  if (!liveToasts.includes(t)) return; // dismissed while the write was in flight
  t.busy = false;
  if (failure === null) {
    t.text = t.undone;
    t.kind = "ok";
    t.undo = null;
  } else {
    t.text = `${original} — undo FAILED: ${failure}`;
    t.kind = "err";
  }
  t.remaining = TOAST_MS;
  armToast(t);
  syncToasts();
}

// ── v11/v12: THE MACRO EDITOR — the piano roll, and it SAVES ───────────────
// docs/INPUT-TRANSFORMS.md §6.2 (TAStudio, adopted): rows = steps, columns =
// the slot's controls, cells = held or not. A timed sequence is a SHAPE, and
// an "add step" form hides it.
//
// v12 wires the write path that landed after this card shipped: the daemon's
// `map-macro` verb (= `ksx macro`, = `ControlSource::save_macro`, =
// `POST /api/macro/save`) takes ONE WHOLE `[macros.<name>]` table. So the grid
// is no longer a copy-and-paste composer — New, Save, Rename and Delete are
// real writes to the preset file, through the same verb the CLI uses. The TOML
// block stays, collapsed, as the sharing/hand-editing path it always was.
//
// THE SAVE MODEL — explicit Save, not save-on-edit. The rest of this page is
// save-immediately (a bind is one atomic key write), but a macro save is a
// WHOLE-TABLE write that (a) takes a timestamped backup every time and (b) is
// hot-swapped into the running session. Autosaving every painted cell would
// therefore publish a half-authored sequence into a live game and leave one
// backup file per click. A grid edit is also a COMPOSITION — paint, reorder,
// retime — and the unit the user means is the finished sequence. Hence: the
// body (cells, steps, durations, policies) is a draft with a loud dirty
// indicator and one Save button; the STRUCTURAL verbs (New / Rename / Delete)
// are single explicit actions and write straight away. Both report through the
// toast stack with Undo, exactly like every other write on this page.
//
// Every derivation here mirrors render_map.rs; the Rust unit tests pin that
// side, including the sampling floor against ksx-core's own MIN_STEP_MS.

/** Mirror of `ksx_core::MIN_STEP_MS` (§0.2: ~16.7 ms per 60 Hz sample, so ~33
 *  ms is two of them). Pinned against the real constant in render_map.rs. */
const MIN_STEP_MS = 33;

/** The same floor counted the way a frame author counts: two 60 Hz samples.
 *  `framesMs(2)` is exactly [`MIN_STEP_MS`], which is the point — a warning
 *  about a `frames = 1` step that answers in milliseconds is asking its reader
 *  to do the conversion that got them here. */
const MIN_STEP_FRAMES = 2;

/** 60 Hz frames → ms, rounded to nearest ONCE (3 frames is 50 ms, not 51). */
function framesMs(frames: number): number {
  return Math.floor((frames * 1000 + 30) / 60);
}

function requestedMs(step: MacroStepView): number | null {
  if (step.ms !== null && step.frames === null) return step.ms;
  if (step.ms === null && step.frames !== null) return framesMs(step.frames);
  return null; // both, or neither — a fault the editor names, never resolves
}

function effectiveMs(step: MacroStepView): number {
  const ms = requestedMs(step);
  if (ms === null) return 0;
  return step.allow_short || ms >= MIN_STEP_MS ? ms : MIN_STEP_MS;
}

function durationText(step: MacroStepView): string {
  if (step.ms !== null && step.frames === null) return `${step.ms} ms`;
  if (step.ms === null && step.frames !== null) {
    return `${step.frames} fr · ${framesMs(step.frames)} ms`;
  }
  return "—";
}

/** Is this step below the sampling floor at all? (Both spellings, one rule.) */
function stepIsShort(step: MacroStepView): boolean {
  const ms = requestedMs(step);
  return ms !== null && ms < MIN_STEP_MS;
}

/** The INLINE flag — short enough to always fit beside the duration.
 *
 *  IN THE AUTHOR'S OWN UNIT. A `frames = 1` step used to be told "16 ms —
 *  raised to 33 ms", which is a true sentence about a number the author never
 *  typed: it hands them the conversion instead of the answer. A step authored
 *  in frames is answered in frames. */
function stepWarning(step: MacroStepView): string {
  if (step.ms !== null && step.frames !== null) return "two units";
  if (step.ms === null && step.frames === null) return "no duration";
  if (!stepIsShort(step)) return "";
  if (step.frames !== null) {
    const f = step.frames;
    return step.allow_short
      ? `${f} fr — may be missed`
      : `${f} fr — raised to ${MIN_STEP_FRAMES} fr`;
  }
  const ms = step.ms as number;
  return step.allow_short
    ? `${ms} ms — may be missed`
    : `${ms} ms — raised to ${MIN_STEP_MS} ms`;
}

/** The same flag in full, for the row's title. */
function stepWarningLong(step: MacroStepView): string {
  if (step.ms !== null && step.frames !== null) {
    return "says both ms and frames — exactly one, or the file is refused";
  }
  if (step.ms === null && step.frames === null) {
    return "no duration — give it ms or frames (a step with none is refused)";
  }
  if (!stepIsShort(step)) return "";
  if (step.frames !== null) {
    const f = step.frames;
    const each = `${f} frame${f === 1 ? "" : "s"} is shorter than the ${MIN_STEP_FRAMES}-frame ` +
      `floor (${MIN_STEP_MS} ms — a press has to survive two 60 Hz polls)`;
    return step.allow_short
      ? `${each} — allow_short is on, so it runs as written and the game may never see it`
      : `${each} — the game may never see it, so ksx raises this step to ` +
          `${MIN_STEP_FRAMES} frames (${MIN_STEP_MS} ms)`;
  }
  const ms = step.ms as number;
  return step.allow_short
    ? `${ms} ms is shorter than ~2 poll intervals (${MIN_STEP_MS} ms) — allow_short is on, ` +
        "so it runs as written and the game may never see it"
    : `${ms} ms is shorter than ~2 poll intervals (${MIN_STEP_MS} ms) — the game may never ` +
        `see it, so ksx raises this step to ${MIN_STEP_MS} ms`;
}

/** FIX 2 — the question Save asks before it writes a macro with a step the
 *  sampler cannot be relied on to see. Empty when there is nothing to ask.
 *
 *  Never a refusal: a short step is legal, `allow_short` exists precisely so
 *  one can be authored on purpose, and Studio does not get to overrule a file
 *  the loader accepts. But it stopped being a SILENT save, because the advisory
 *  under the grid was read as decoration by the one person it was written for.
 *
 *  The two cases are different consequences and are counted apart: a plain
 *  short step is RAISED (the sequence runs slower than it reads), a short step
 *  marked `allow_short` runs as written and may be MISSED. The floor is quoted
 *  in both units so it lands whichever one the author is thinking in. */
function shortStepQuestion(mac: MacroView): string {
  const raised = mac.steps.filter((s) => stepIsShort(s) && !s.allow_short).length;
  const missable = mac.steps.filter((s) => stepIsShort(s) && s.allow_short).length;
  if (raised + missable === 0) return "";
  const floor = `~${MIN_STEP_MS} ms (${MIN_STEP_FRAMES} frames at 60 Hz)`;
  const parts: string[] = [];
  if (raised > 0) {
    parts.push(
      `${raised} step${raised === 1 ? " is" : "s are"} shorter than ${floor} — a 60 Hz game ` +
        `may never see ${raised === 1 ? "it" : "them"}, so ksx will run ` +
        `${raised === 1 ? "it" : "each of them"} for ${MIN_STEP_MS} ms instead of the ` +
        "time you wrote",
    );
  }
  if (missable > 0) {
    parts.push(
      `${missable} step${missable === 1 ? " is" : "s are"} shorter than ${floor} AND marked ` +
        `allow short — ${missable === 1 ? "it runs" : "they run"} exactly as written, so a ` +
        `60 Hz game may never see ${missable === 1 ? "it" : "them"} at all`,
    );
  }
  return `${parts.join(". ")}. Save anyway?`;
}

function macroTotalMs(mac: MacroView): number {
  return mac.steps.reduce((sum, s) => sum + effectiveMs(s), 0);
}

// ── v12: the frame arithmetic, on screen ───────────────────────────────────
// Victor asked the question this answers: "a 60fps frame is only like
// sixteenth milliseconds? maybe we can show that math." So wherever a duration
// is edited the conversion is printed live, with the sampling floor in the
// SAME units — which makes "too short" self-explanatory instead of a rule to
// remember.
//
// The target rate is DISPLAY-ONLY, deliberately. Many arcade titles are not
// exactly 60 Hz (59.94, 57, 55 are common), so authoring against the game's
// real rate is genuinely useful — but neither the preset file
// (`ksx_config::MacroStepFile` = hold / ms / frames / allow_short) nor the
// `map-macro` wire body (`MacroWrite`) has anywhere to store a rate, and
// `ksx_core::StepDuration::Frames` counts frames at 60 Hz FULL STOP. Inventing
// a field the daemon would drop is the silent-no-op this page bans. So the
// selector converts for the author and says, in words, that a `frames = N`
// step still runs at 60 Hz — and offers the ms value that matches the game.

/** The rate the AUTHOR is thinking in. Never written anywhere; see above. */
let macroRateHz = 60;

export function macroTargetRate(): number {
  return macroRateHz;
}

/** `60`, `59.94`, `57`… — anything else is ignored rather than turned into a
 *  divide-by-zero in the line below. */
export function setMacroTargetRate(hz: number): void {
  if (!Number.isFinite(hz) || hz <= 0) return;
  macroRateHz = hz;
  refreshMacro();
}

function hz(rate: number): string {
  return `${Number.isInteger(rate) ? rate : rate.toFixed(2)} Hz`;
}

/** The floor, in the author's own units: "33 ms (2.0 frames @ 60 Hz)". */
function floorText(rate: number): string {
  return `${MIN_STEP_MS} ms (${((MIN_STEP_MS * rate) / 1000).toFixed(1)} frames @ ${hz(rate)})`;
}

/** The live conversion for ONE step. Mirrored in render_map.rs `frame_math`. */
function frameMath(step: MacroStepView | undefined, rate: number): string {
  const floor = `The engine can only see steps of ${floorText(rate)} or longer.`;
  if (!step) return `Pick a step's ⏱ to retime it. ${floor}`;
  if (step.ms !== null && step.frames !== null) {
    return `This step says both ms and frames — keep exactly one, or the preset will not load. ${floor}`;
  }
  if (step.ms === null && step.frames === null) {
    return `This step has no duration — give it ms or frames. ${floor}`;
  }
  if (step.frames !== null) {
    const f = step.frames;
    const ksx = framesMs(f);
    if (rate === 60) {
      return `${f} frame${f === 1 ? "" : "s"} @ 60 Hz = ${ksx.toFixed(1)} ms. ${floor}`;
    }
    const atRate = (f * 1000) / rate;
    return (
      `${f} frame${f === 1 ? "" : "s"} @ ${hz(rate)} = ${atRate.toFixed(1)} ms — but ksx counts ` +
      `frames at 60 Hz, so this step runs ${ksx.toFixed(1)} ms. To match the game, switch the ` +
      `unit to ms and enter ${Math.round(atRate)}. ${floor}`
    );
  }
  const ms = step.ms as number;
  return `${ms} ms = ${((ms * rate) / 1000).toFixed(1)} frames @ ${hz(rate)}. ${floor}`;
}

/** The macro's REPEAT arithmetic, in words — the same treatment the duration
 *  field got, and for the same reason: `turbo_hz = 30` on a 50 ms sequence is
 *  not 30 Hz and never could be. Mirrored in render_map.rs `turbo_math`. */
function turboMath(mac: MacroView | null): string {
  if (mac === null) return "";
  if (mac.repeat === "while-held") {
    return (
      "Holding the trigger starts the sequence again the instant it ends, with NO gap " +
      "between runs — the right shape for a MOTION whose last step flows into its first, " +
      "and the wrong one for auto-fire (a game reads two touching runs as one long hold)."
    );
  }
  if (mac.repeat !== "turbo") {
    return (
      "One run per press. Holding the trigger changes nothing, which is what stops a " +
      "special move turning into a machine gun when a panel switch bounces."
    );
  }
  const run = macroTotalMs(mac);
  let asked: string;
  let wanted: number;
  if (mac.turbo_hz !== null) {
    asked = `Requested ${mac.turbo_hz} Hz`;
    const hz = Math.min(Math.max(mac.turbo_hz, 1), TURBO_MAX_HZ);
    wanted = Math.max(Math.floor((1000 + Math.floor(hz / 2)) / hz) - run, 0);
  } else if (mac.gap_ms !== null) {
    asked = `Requested a ${mac.gap_ms} ms gap`;
    wanted = mac.gap_ms;
  } else {
    asked = "No rate given — a turbo with no rate is refused by the loader";
    wanted = MIN_STEP_MS;
  }
  const raised = wanted < MIN_STEP_MS;
  const gap = raised ? MIN_STEP_MS : wanted;
  const cycle = run + gap;
  if (cycle === 0) return "This macro has no steps, so there is nothing to repeat.";
  const effective = Math.floor((1000 + Math.floor(cycle / 2)) / cycle);
  const why = raised
    ? " (raised to the sampling floor — a gap the game never samples is not a gap, it " +
      "reads as one long hold)"
    : "";
  return (
    `${asked} → effective ~${effective} Hz, because the sequence itself is ${run} ms long ` +
    `and the neutral gap between runs is ${gap} ms${why}: one full press/release cycle ` +
    `takes ${cycle} ms. Each half has to survive a 60 Hz poll (${MIN_STEP_MS} ms), which ` +
    "is what caps this — the rate is capped, never refused."
  );
}

const MACRO_RULE_LINE =
  "Amber steps are shorter than ~2 poll intervals — 33 ms, or 2 frames if you are counting " +
  "frames — which is the shortest thing a game can be relied on to see. A 1-frame step is " +
  "not unreliable, it is invisible. ksx raises a short step to 33 ms so it lands; a step " +
  "marked allow_short runs exactly as written and can be missed entirely. Neither is ever " +
  "silent, and Save asks before it writes either one.";

/** THE RING, stated once, under the grid: what the eight columns of a direction
 *  group are, what each is called, and — for the four that are PICKS rather than
 *  stored names — exactly what ksx writes when you tick one. Mirror of
 *  render_map.rs `MACRO_RING_LINE`.
 *
 *  The numpad digits live HERE and in the tooltips, never in the glyph row: a
 *  second line of digits under only 24 of 37 columns makes the header ragged,
 *  and the digit is a lookup key (for somebody who read "3" on Dustloop or typed
 *  digits into MAME's `joystick_map`), not a label. */
const MACRO_RING_LINE =
  "Each direction group runs ↑ ↖ ← ↙ ↓ ↘ → ↗ (numpad 8 7 4 1 2 3 6 9), so a motion is a " +
  "SHAPE: a quarter-circle forward is a staircase, a half-circle a straight line, a dragon " +
  "punch a hook. The four diagonals are picks, not new bindings — ticking ↘ (down-right, " +
  "numpad 3; a move list spells it d/f, which is only down-FORWARD while you face right — ksx " +
  "has no idea which way you are facing) stores dpad.down + dpad.right on that step, which is " +
  "what a diagonal has always been in this file. THERE ARE THREE OF THESE GROUPS — D-PAD, " +
  "LEFT STICK and RIGHT STICK — and the grid scrolls sideways to reach them, so the one you " +
  "want may be off the edge; the band above the arrows names whichever you are looking at. " +
  "Tick the diagonal on the group your preset's own direction keys drive, because a motion " +
  "written on the other one is published faithfully and read by nobody. Each row spells the " +
  "pair it wrote beside its name.";

/** The body "＋ New macro" WRITES: one real 50 ms step, at the default
 *  policies. A macro with no steps is refused by the loader (and by the
 *  daemon), so a new table has to arrive with one — and one empty step is the
 *  honest starting point, because the grid below it is where the holds are
 *  painted. There is no browser-only draft version of this: the macro exists
 *  in the preset the moment the button lands, which is what makes its trigger
 *  bindable. */
export function newMacroBody(name: string): MacroView {
  return {
    name,
    steps: [{ hold: [], ms: 50, frames: null, allow_short: false }],
    on_release: "finish",
    retrigger: "ignore",
    interrupt: "none",
    // A new macro runs ONCE. Auto-fire is asked for by name, never a default
    // a starter body hands somebody who did not ask for it.
    repeat: "once",
    turbo_hz: null,
    gap_ms: null,
    triggers: [],
  };
}

/** The two spellings a duration can be authored in (§1c). */
type StepUnit = "ms" | "frames";

/** The unit each step was AUTHORED in — its own state, remembered per step.
 *
 *  Keyed by the STEP OBJECT rather than by an index, so the choice follows the
 *  step through every move / insert / delete without a parallel array anybody
 *  has to remember to splice in step.
 *
 *  Why it is state and not a derivation: the editor used to read the unit back
 *  off the value ("`frames` is not null? then frames, else ms"), which means a
 *  step that is not there — or a value normalised anywhere between here and
 *  the file — answers "ms". That is exactly how a unit the author picked
 *  turned back into ms on its own. The value is the file's; the unit is the
 *  author's, and this is where the author's half lives. */
const stepUnits = new WeakMap<MacroStepView, StepUnit>();

/** The unit the duration control shows when NO step is selected: the last one
 *  the author actually picked, never a default that overwrites their choice. */
let macroLastUnit: StepUnit = "ms";

/** How the FILE spells this step's duration. Reading the preset's own shape is
 *  not inference — `ms` and `frames` are kept apart on disk precisely so an
 *  authored unit round-trips (§1c) — but it is the ONLY place a unit is ever
 *  read from a value, and it happens once, when a draft is seeded. */
function fileUnitOf(step: MacroStepView): StepUnit {
  return step.frames !== null && step.ms === null ? "frames" : "ms";
}

/** The unit ONE step is authored in — the author's choice when they made one,
 *  the file's spelling otherwise. Per step, because since FIX 2 every row
 *  carries its own duration control and there is no "the selected step" left
 *  to ask about. */
function unitOfStep(step: MacroStepView): StepUnit {
  return stepUnits.get(step) ?? fileUnitOf(step);
}

function cloneMacro(mac: MacroView): MacroView {
  return {
    ...mac,
    steps: mac.steps.map((s) => {
      const copy = { ...s, hold: [...s.hold] };
      stepUnits.set(copy, fileUnitOf(s));
      return copy;
    }),
    triggers: [...mac.triggers],
  };
}

// ── Draft state (client-only) ──────────────────────────────────────────────
// The draft belongs to ONE preset and ONE macro. A poll re-seeds it only while
// it is untouched, so a 2 s refresh can never eat an edit in progress — but
// the TRIGGER is always re-read from the file, because that half really is
// saved and the draft has no business remembering a stale version of it.

let macroDraft: MacroView | null = null;
/** The draft came from a `[macros]` table that exists on disk. Since v12 that
 *  is true of every draft this page can produce — the only way to get a new
 *  macro is to CREATE it — but it stays as the guard that keeps Save, Rename
 *  and Delete pointed at something the preset really holds. */
let macroFromDisk = false;
/** The name it was seeded FROM — what "Revert to file" goes back to. */
let macroSeedName: string | null = null;
/** The macro the USER is looking at, which a poll must not change. */
let macroChosen: string | null = null;
/** The grid differs from the file: there is something for Save to write. */
let macroDirty = false;
/** Which step the duration editor is pointed at. */
let macroStep: number | null = null;
/** A macro-editor control has the caret right now — an edit in progress, which
 *  no poll and no hover may repaint out from under. map.ts drives this from
 *  focusin/focusout: the island holds the state, the page holds the DOM. */
let macroEditorFocused = false;
/** FIX 2: Save has ASKED about this draft's short steps and is waiting for an
 *  answer. Cleared by any edit and by every macro switch — the question is
 *  about the steps as they stood when it was asked, and an armed "Save anyway"
 *  surviving an edit would be a button that writes something nobody read. */
let macroSaveAsked = false;

/** Is there an edit in flight the poll must leave alone? Unsaved changes, or a
 *  control the user's hands are on this second. */
function macroEditorBusy(): boolean {
  return macroDirty || macroEditorFocused;
}

export function setMacroEditorFocused(on: boolean): void {
  macroEditorFocused = on;
}

export function currentMacro(): MacroView | null {
  return macroDraft;
}

/** This preset's macro NAMES, as the file spells them. */
export function macroNames(): string[] {
  return (lastPayload?.macros.macros ?? []).map((m) => m.name);
}

/** The macro as it is ON DISK right now — what an undo has to put back, read
 *  before the write like every other undo on this page. */
export function macroOnDiskCopy(name: string): MacroView | null {
  const found = (lastPayload?.macros.macros ?? []).find(
    (m) => m.name.toLowerCase() === name.toLowerCase(),
  );
  return found ? cloneMacro(found) : null;
}

/** Is the draft a table the preset actually holds? */
export function macroIsOnDisk(): boolean {
  return macroFromDisk;
}

/** What is wrong with `name` as a macro name, in one sentence — or null.
 *
 *  The name is half of the `macro.<name>` function that starts the sequence
 *  and it is a TOML table key, so the vocabulary is kept to what survives both
 *  without quoting: letters, digits, dash, underscore, dot. The daemon
 *  validates for itself and its refusal is what lands on screen; this is the
 *  local half, so an obvious mistake is answered before a round trip. */
export function macroNameProblem(name: string, except?: string | null): string | null {
  const clean = name.trim();
  if (clean === "") {
    return "A macro needs a name — it is half of the `macro.<name>` key that starts it.";
  }
  if (clean.length > 64) return "That name is longer than 64 characters.";
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(clean)) {
    return (
      `"${clean}" has characters a macro name cannot use. Use letters, digits, dash, ` +
      "underscore or dot, starting with a letter or digit — the name is a TOML table key " +
      "and half of the `macro.<name>` function, and both have to hold it without quoting."
    );
  }
  const taken = macroNames().find(
    (n) => n.toLowerCase() === clean.toLowerCase() && n.toLowerCase() !== (except ?? "").toLowerCase(),
  );
  if (taken !== undefined) {
    return `"${taken}" already exists in this preset. Pick another name, or open that macro and edit it.`;
  }
  return null;
}

export function currentMacroStep(): number | null {
  return macroStep;
}

export function macroIsDirty(): boolean {
  return macroDirty;
}

/** The key(s) that start `macro.<name>` right now, from the FILE. */
export function macroTriggersOf(fn: string): string[] {
  const name = fn.startsWith("macro.") ? fn.slice("macro.".length) : fn;
  const found = lastPayload?.macros.macros.find(
    (m) => m.name.toLowerCase() === name.toLowerCase(),
  );
  return found ? [...found.triggers] : [];
}

/** Point the editor at one of the preset's macros, discarding whatever draft
 *  was open.
 *
 *  v12: a name that matches nothing leaves the editor EMPTY rather than
 *  inventing a draft. The old fallback minted a browser-only "my-macro" whose
 *  trigger could not be bound ("preset defines no macro called my-macro") —
 *  the exact confusion this rewrite exists to remove. The way to get a new
 *  macro is now "＋ New macro", which writes one. */
export function seedMacro(name: string | null): void {
  const list = lastPayload?.macros.macros ?? [];
  // `null` = "whatever the user is looking at", which is what every 2 s poll
  // asks for. Remembering it here is what stops a poll from yanking the editor
  // back to the preset's FIRST macro two seconds after a tab click — the same
  // snap-back that made a rename look like it "just resets".
  const want = (name ?? macroChosen ?? lastPayload?.macro_selected ?? "").toLowerCase();
  const found = list.find((m) => m.name.toLowerCase() === want) ?? (name === null ? list[0] : undefined);
  const wasChosen = macroChosen;
  const wasStep = macroStep;
  macroChosen = found ? found.name : null;
  macroDraft = found ? cloneMacro(found) : null;
  macroFromDisk = found !== undefined;
  macroSeedName = found ? found.name : null;
  macroDirty = false;
  macroSaveAsked = false;
  // WHICH STEP the editor points at is the USER's place in the macro, not the
  // file's — so re-seeding the SAME macro keeps it. The 2 s poll re-seeds
  // every clean draft, and clearing the selection there is what made the
  // duration editor let go on its own: with no step to edit, the unit control
  // has nothing to describe, `Set unit` finds nothing to set, and the next
  // sync writes "ms" back over the author's pick. A DIFFERENT macro is a
  // different sequence, so that still starts with nothing selected.
  macroStep =
    found !== undefined &&
    wasChosen !== null &&
    found.name.toLowerCase() === wasChosen.toLowerCase() &&
    wasStep !== null &&
    wasStep < found.steps.length
      ? wasStep
      : null;
  refreshMacro();
}

/** The write landed: this draft IS the file now. Called by map.ts after a
 *  successful save so the dirty flag clears without waiting for the poll (and
 *  so the poll's re-seed, which only fires on a clean draft, takes over). */
export function markMacroSaved(name: string): void {
  if (macroDraft) macroDraft.name = name;
  macroFromDisk = true;
  macroSeedName = name;
  macroChosen = name;
  macroDirty = false;
  macroSaveAsked = false;
  refreshMacro();
}

/** The `[macros]` table this draft came from — "Revert to file"'s target,
 *  which a rename must not lose. `null` = it came from nothing. */
export function macroSeededFrom(): string | null {
  return macroSeedName;
}

/** A draft belongs to one preset, so a slot switch drops it — the same rule
 *  the multi-select follows. */
export function resetMacroDraft(): void {
  macroDraft = null;
  macroFromDisk = false;
  macroSeedName = null;
  macroChosen = null;
  macroDirty = false;
  macroStep = null;
  macroLastUnit = "ms";
  macroSaveAsked = false;
}

/** Every mutation lands here: mark the draft edited and repaint. */
function macroEdited(): void {
  macroDirty = true;
  // FIX 2: an edit answers Save's question by changing the thing it was about.
  // Leaving "Save anyway?" armed across an edit would arm a button for a draft
  // nobody has been asked about.
  macroSaveAsked = false;
  refreshMacro();
}

export function macroSelectStep(index: number): void {
  const mac = macroDraft;
  if (!mac || index < 0 || index >= mac.steps.length) return;
  macroStep = index;
  refreshMacro();
}

/** Paint (or clear) one cell of the roll — the whole point of the shape.
 *
 *  Three kinds of column, one entry point:
 *
 *  - **A DIAGONAL PICK** (`diag:<mech>:<diag>`) is expanded here and nowhere
 *    else. Ticking it stores the PAIR, on that column's own mechanism — which
 *    is what makes it mechanism-aware and why it can never raise
 *    `Issue::MacroHoldsOtherMechanism` against a group the user pointed at.
 *    Untick removes exactly the two. Asking for ↘ on a mechanism that is
 *    already pointing somewhere else on the same axis REPLACES that direction:
 *    "make this mechanism point ↘" is the whole meaning of the click, and
 *    leaving `dpad.up` in place would produce a contradictory step that folds
 *    to nothing and shows the cell still off. Every other hold — the other
 *    mechanisms, the buttons, `consume` — is untouched.
 *  - **A DIRECTION** matches by where it POINTS, so ticking `ly.min` off a step
 *    that spells it `ly.-16384` clears the one that is really there.
 *  - **Anything else** is the plain string toggle it always was.
 *
 *  Returns what changed, in words, plus the exact hold it replaced — so the
 *  toast can offer Undo, the same parity every other write on this page has.
 *  Null when there is nothing to report (a plain single-control toggle, which
 *  the grid already shows). */
export function macroToggleCell(index: number, fn: string): MacroCellOutcome | null {
  const step = macroDraft?.steps[index];
  if (!step) return null;
  // The undo below closes over the STEP OBJECT, not this index — the index is
  // only good for the length of the click. See `macroRestoreHold`.
  const before = [...step.hold];
  const d = parseDiagToken(fn);
  if (d) {
    const [up, right] = DIAG_HALVES[d.diag];
    const pair = [
      mechanismFunction(d.mechanism, true, up),
      mechanismFunction(d.mechanism, false, right),
    ];
    // IS THIS CELL LIT? Asked of `fold` — the SAME function that painted it —
    // and never of "does the hold contain both halves".
    //
    // THE BUG THAT SETTLES IT (found by driving the real page): those two
    // questions disagree on exactly one shape, and it is a shape real files
    // carry. `dpad.down + dpad.right + dpad.up` CONTAINS both halves of ↘, but
    // it does not fold — it contradicts itself, so the cell paints OFF and its
    // own title says "step 4 does not hold D-pad ↘". Under "contains both",
    // one click on that off cell took the untick branch: it wiped all three
    // holds, left the step empty, and reported *"cleared D-pad ↘ — removed
    // dpad.down + dpad.right"*. A cell that says it does not hold ↘, answering
    // a click by clearing ↘, naming two of the three things it took, and
    // handing back a step that holds nothing — in the one feature whose whole
    // job is that picking a diagonal gives you a diagonal.
    //
    // Paint and click now read one function, so a cell can only ever do what
    // it looks like it will do: off → tick it on, lit → clear it.
    const on = fold(step.hold).some(
      (h) => h.kind === "diag" && h.diag === d.diag && h.mechanisms.includes(d.mechanism),
    );
    // Everything this mechanism holds RIGHT NOW, however the file spells it —
    // what an untick really removes and what a tick really replaces.
    const mine = before.filter((f) => pointing(f)?.mechanism === d.mechanism);
    // Drop every direction this mechanism is currently holding, then (when
    // this was a tick rather than an untick) say the diagonal.
    step.hold = step.hold.filter((f) => {
      const p = pointing(f);
      return p === null || p.mechanism !== d.mechanism;
    });
    if (!on) step.hold.push(...pair);
    macroStep = index;
    macroEdited();
    const glyph = DIAG_GLYPH[d.diag];
    // Named from the hold as WRITTEN, not from the canonical pair: unticking a
    // hand-written `ly.-16384 + lx.max` must not claim it removed `ly.min`.
    const displaced = mine.filter(
      (f) => !pair.some((p) => p.toLowerCase() === f.toLowerCase()),
    );
    const said = on
      ? `Step ${index + 1}: cleared ${mechanismGroup(d.mechanism)}${glyph} — removed ` +
        `${mine.join(" + ")}.`
      : `Step ${index + 1}: ${mechanismGroup(d.mechanism)}${glyph} (${DIAG_WORDS[d.diag]}, ` +
        `numpad ${DIAG_NUMPAD[d.diag]}) — ksx wrote ${pair.join(" + ")}, because that is ` +
        `what a diagonal is in the file.` +
        (displaced.length > 0
          ? ` Replaced ${displaced.join(" + ")} on ${mechanismLabel(d.mechanism)}.`
          : "");
    return { said, undo: () => macroRestoreHold(step, before) };
  }

  const want = pointing(fn);
  const at =
    want === null
      ? step.hold.findIndex((f) => f.toLowerCase() === fn.toLowerCase())
      : step.hold.findIndex((f) => pointsSameWay(f, want));
  if (at >= 0) step.hold.splice(at, 1);
  else step.hold.push(fn);
  macroStep = index;
  macroEdited();
  // THE FOLD MOMENT — see `shapeChange`. A plain cardinal toggle usually says
  // nothing, because the cell you hit is the whole report. The exception is the
  // click that makes or breaks a diagonal, where the cell you hit is precisely
  // NOT the report.
  const said = shapeChange(index, before, step.hold);
  return said === null ? null : { said, undo: () => macroRestoreHold(step, before) };
}

/** Did this click change the SHAPE of the row — did two cardinals just become a
 *  diagonal, or did a diagonal just come apart?
 *
 *  **This is the sentence Victor lost an evening to, and it was the one report
 *  nobody got.** Ticking `→` on a step already holding `↓` does four things at
 *  once: the cell just clicked goes to a subordinate `·` instead of a filled
 *  mark, a cell twelve columns away lights up, the row's words change from
 *  "D-pad ↓" to "D-pad ↘", and a ledger appears. All of it silent. What a user
 *  can reasonably conclude from that is either that the grid is broken or that
 *  ksx quietly rewrote their input — and the truth, that their two holds ARE a
 *  diagonal and the file still spells both, is the one thing the page never
 *  said out loud.
 *
 *  Note which way round this is. PICKING ↘ is self-explanatory — you clicked ↘
 *  and you got ↘; the report there is a courtesy. The FOLD is the surprising
 *  one, so the fold is what has to speak.
 *
 *  Null when the row's shape is unchanged, which is the overwhelmingly common
 *  case (every button, every cardinal that neither completes nor breaks a
 *  pair). */
function shapeChange(index: number, before: string[], after: string[]): string | null {
  const shapeOf = (hold: string[]): Map<Mechanism, { diag: Diag; names: string[] }> => {
    const out = new Map<Mechanism, { diag: Diag; names: string[] }>();
    for (const h of fold(hold)) {
      if (h.kind !== "diag") continue;
      const names = h.members.map((m) => hold[m]);
      for (const mechanism of h.mechanisms) {
        out.set(
          mechanism,
          // A coalesced diagonal lists every mechanism's members together, so
          // narrow the names back down to the ones this mechanism owns.
          { diag: h.diag, names: names.filter((n) => pointing(n)?.mechanism === mechanism) },
        );
      }
    }
    return out;
  };
  const was = shapeOf(before);
  const now = shapeOf(after);
  const said: string[] = [];
  for (const [mechanism, made] of now) {
    if (was.get(mechanism)?.diag === made.diag) continue;
    said.push(
      `${mechanismGroup(mechanism)}${DIAG_GLYPH[made.diag]} — holding ` +
        `${made.names.join(" and ")} at the same time IS the diagonal ` +
        `(${DIAG_WORDS[made.diag]}, numpad ${DIAG_NUMPAD[made.diag]}), so the row now reads it ` +
        `as one control. Nothing was rewritten: the file still says ${made.names.join(" + ")}.`,
    );
  }
  for (const [mechanism, lost] of was) {
    if (now.get(mechanism)?.diag === lost.diag) continue;
    const left = after.filter((f) => pointing(f)?.mechanism === mechanism);
    said.push(
      `${mechanismGroup(mechanism)}${DIAG_GLYPH[lost.diag]} is no longer a diagonal — ` +
        (left.length === 0
          ? `this step holds no direction on ${mechanismLabel(mechanism)} now.`
          : `${mechanismLabel(mechanism)} is left holding ${left.join(" + ")}, which ` +
            `${left.length === 1 ? "is one direction, not two" : "does not point one way"}.`),
    );
  }
  return said.length === 0 ? null : `Step ${index + 1}: ${said.join(" ")}`;
}

/** What a cell click did, and the road back. */
export interface MacroCellOutcome {
  said: string;
  /** Puts the step's hold back exactly as it was. Returns the refusal when the
   *  draft moved on underneath it (a slot switch, a revert) — never a silent
   *  no-op, same rule as every other undo here. */
  undo: () => string | null;
}

/** Restore one step's hold, byte for byte. The undo half of a diagonal pick.
 *
 *  **Addressed by the STEP OBJECT, never by its index.** An undo is a closure
 *  that outlives its click by `TOAST_MS`, and in those eight seconds the draft
 *  it is about can be replaced entirely — a macro tab, a slot switch and
 *  "Revert to file" all call `seedMacro`, which the editor allows while dirty
 *  (it warns, and discards). "Does step 1 exist?" is true in the NEXT macro
 *  too, so an index-addressed undo answered by writing one sequence's hold into
 *  a different sequence's step 1: silently, on a row the user was looking at,
 *  marking that macro dirty, and reporting "Undone."
 *
 *  The object is the exact test, because `cloneMacro` mints fresh step objects
 *  on every seed — so any re-seed misses, and a step that merely MOVED (⬆/⬇, an
 *  insert above) is still found, at its new index, which is the row that undo
 *  was always about. Same WeakMap-keyed-by-step reasoning as [`stepUnits`]. */
export function macroRestoreHold(target: MacroStepView, hold: string[]): string | null {
  const at = macroDraft?.steps.indexOf(target) ?? -1;
  if (at < 0) {
    return (
      "that step is gone — the draft was reloaded (a macro switch, a slot switch, or " +
      "Revert to file), so there is nothing to put back."
    );
  }
  target.hold = [...hold];
  macroStep = at;
  macroEdited();
  return null;
}

function newStep(): MacroStepView {
  const step: MacroStepView = { hold: [], ms: 50, frames: null, allow_short: false };
  stepUnits.set(step, "ms");
  return step;
}

// ── FIX 1c: COMMON MOTIONS — the sequences everyone is actually building ────
// WHY THIS EXISTS. The quarter-circle is where both traps bite at once, and
// they bite together for the same reason: a motion is not a list of directions,
// it is a list of STATES, and the state in the middle is two directions held
// together. Somebody who has not learned that writes ↓ then → then punch, gets
// nothing, and — reasonably — goes looking for a timing bug. Handing them a
// correct three-step group with `↓ + →` written on the middle row teaches the
// concept in one click, on their own macro, with their own pad's names on it.
//
// It generates from the SLOT'S OWN BINDINGS, never from a fixed table: a pad
// has three ways to say "right" (dpad, left stick, right stick) and ksx
// publishes exactly what a step names, so a motion written in dpad holds on a
// preset whose player keys drive the left stick is published faithfully and
// read by nobody. That is `Issue::MacroHoldsOtherMechanism` in
// ksx-config/src/validate.rs — an advisory that arrives AFTER a save. Choosing
// the mechanism the preset already drives means the generated steps never
// raise it. Same rule as `driven_mechanisms` there: an inert `None` row is a
// function the preset lists, not a direction the player can produce.
//
// Durations default to 50 ms — above the ~33 ms floor, deliberately: a helper
// that seeds steps the sampler cannot see would be this card teaching the exact
// mistake it exists to prevent.

/** Which control a preset's direction keys drive. Mirror of
 *  `ksx_core::socd::DirMechanism` (= `ksx_config::validate::Mechanism`, which
 *  is now a re-export of it) and of render_map.rs `Mechanism`. */
type Mechanism = "dpad" | "lstick" | "rstick";

/** Canonical order — how a coalesced diagonal lists its mechanisms and how the
 *  grid draws its direction groups. */
const MECHANISMS: Mechanism[] = ["dpad", "lstick", "rstick"];

function mechanismOf(fn: string): Mechanism | null {
  const f = fn.toLowerCase();
  if (f.startsWith("dpad.")) return "dpad";
  if (f.startsWith("lx.") || f.startsWith("ly.")) return "lstick";
  if (f.startsWith("rx.") || f.startsWith("ry.")) return "rstick";
  return null;
}

function mechanismLabel(m: Mechanism): string {
  if (m === "dpad") return "the dpad";
  return m === "lstick" ? "the left stick (lx/ly)" : "the right stick (rx/ry)";
}

/** The prefix a flat list needs to keep three identical arrow runs apart — the
 *  same one `legendGroup` writes. */
function mechanismGroup(m: Mechanism): string {
  if (m === "dpad") return "D-pad ";
  return m === "lstick" ? "LS " : "RS ";
}

/** The grid's group-band label. */
function mechanismBand(m: Mechanism): string {
  if (m === "dpad") return "D-PAD";
  return m === "lstick" ? "LEFT STICK" : "RIGHT STICK";
}

/** The half of a diagonal cell token that names the mechanism. */
function mechanismToken(m: Mechanism): string {
  return m === "lstick" ? "ls" : m === "rstick" ? "rs" : "dpad";
}

/** The canonical function name for one polarity of one axis of a mechanism —
 *  what picking a direction WRITES. Mirror of `Mechanism::function`. */
function mechanismFunction(m: Mechanism, vertical: boolean, positive: boolean): string {
  if (m === "dpad") {
    return vertical ? (positive ? "dpad.up" : "dpad.down") : positive ? "dpad.right" : "dpad.left";
  }
  const [h, v] = m === "lstick" ? ["lx", "ly"] : ["rx", "ry"];
  return vertical ? `${v}.${positive ? "max" : "min"}` : `${h}.${positive ? "max" : "min"}`;
}

// ── DIAGONALS AS PRESENTATION ─────────────────────────────────────────────
// Victor: "if down and right together equals diagonal, the user does not care —
// we can present the diagonal in the piano and the user can select it, and
// behind the scenes we do down and right, so it's seamless."
//
// He is right, and it is the root cause of the evening he lost. A diagonal IS
// two simultaneous holds — ksx's implementation detail, not the user's concept.
// Players think in ↘ / down-forward / numpad 3, never in "two axis bindings held
// together", and no mapper in the field lets them pick one (Steam Input: four
// cardinal binding slots; reWASD's own answer: build a Shortcut out of two
// zones; MAME: four cardinals; GP2040-CE: cardinal pairs, with its whole SOCD
// feature spent on what happens when the pair is illegal).
//
// NOTHING STORED CHANGES. A step still holds a set of ordinary bindings, so the
// file stays hand-editable, the engine is untouched, and old presets keep
// working. `fold` reads a hold and says how to PRESENT it; a pick writes the
// pair, spelled exactly as `ksx map` would.
//
// MIRROR of `ksx_core::diagonal` + `ksx_core::socd::pointing`, over FUNCTION
// NAMES rather than Binding — same rule as the zone tables above, and pinned
// against the Rust side by render_map.rs `the_diagonal_lens_matches_ksx_core`.

type Diag = "ul" | "ur" | "dl" | "dr";

/** Canonical order. */
const DIAGS: Diag[] = ["ul", "ur", "dl", "dr"];

/** ARROW is the glyph. Screens speak arrows in this genre — SF6's input
 *  history, every Capcom move list, every arcade instruction card. */
const DIAG_GLYPH: Record<Diag, string> = { ul: "↖", ur: "↗", dl: "↙", dr: "↘" };

/** A LOOKUP TOKEN for tooltips and the ring line, never the label: it is how
 *  the input is written in TEXT (Dustloop, SuperCombo), and it is already in a
 *  cab owner's `mame.ini` — `-joystick_map`'s digits use the numpad mapping. */
const DIAG_NUMPAD: Record<Diag, number> = { ul: 7, ur: 9, dl: 1, dr: 3 };

/** COMPASS, not forward/back: ksx offers the mirrored spelling of every motion
 *  because player 2 is not an edge case, and "down-forward" is only true for a
 *  character facing right. */
const DIAG_WORDS: Record<Diag, string> = {
  ul: "up-left",
  ur: "up-right",
  dl: "down-left",
  dr: "down-right",
};

/** How a move list writes it (Tekken's command lists spell `d/f`). Offered
 *  BESIDE the compass name, never instead of it. */
const DIAG_MOVELIST: Record<Diag, string> = { ul: "u/b", ur: "u/f", dl: "d/b", dr: "d/f" };

/** `[up, right]`. */
const DIAG_HALVES: Record<Diag, [boolean, boolean]> = {
  ul: [true, false],
  ur: [true, true],
  dl: [false, false],
  dr: [false, true],
};

function diagFromHalves(up: boolean, right: boolean): Diag {
  return up ? (right ? "ur" : "ul") : right ? "dr" : "dl";
}

/** The glyph for one cardinal polarity. ARROWS, the same family the diagonals
 *  wear — a diagonal that does not look like the same family as its two parents
 *  defeats the whole lens, and `◤◥◣◢` are corner *blocks*, not directions. */
function cardinalGlyph(vertical: boolean, positive: boolean): string {
  return vertical ? (positive ? "↑" : "↓") : positive ? "→" : "←";
}

function cardinalWords(vertical: boolean, positive: boolean): string {
  return vertical ? (positive ? "up" : "down") : positive ? "right" : "left";
}

function cardinalNumpad(vertical: boolean, positive: boolean): number {
  return vertical ? (positive ? 8 : 2) : positive ? 6 : 4;
}

/** Mirror of `ksx_core::socd::Pointing`, over a FUNCTION NAME. */
interface Pointing {
  mechanism: Mechanism;
  vertical: boolean;
  /** Right for a horizontal control, UP for a vertical one. */
  positive: boolean;
  /** Canonical extreme (`min`/`max`), or a hand-written partial deflection? */
  exact: boolean;
}

/** Where this function name points, or null when it points nowhere.
 *
 *  A CENTRED AXIS IS NEVER A DIRECTION (`lx.0`) — the same rule
 *  `ksx_core::socd::pointing` and `ksx_config::validate` state, which is why it
 *  is never half of a diagonal either. */
function pointing(fn: string): Pointing | null {
  const lower = fn.toLowerCase();
  const at = lower.indexOf(".");
  if (at < 0) return null;
  const base = lower.slice(0, at);
  const rest = lower.slice(at + 1);
  if (base === "dpad") {
    const table: Record<string, [boolean, boolean]> = {
      up: [true, true],
      down: [true, false],
      left: [false, false],
      right: [false, true],
    };
    const hit = table[rest];
    if (!hit) return null;
    return { mechanism: "dpad", vertical: hit[0], positive: hit[1], exact: true };
  }
  if (base !== "lx" && base !== "ly" && base !== "rx" && base !== "ry") return null;
  // `min` / `max` / `<i16>` — the same grammar `ksx_config::parse_function`
  // takes, including its i16::MIN fold.
  let value: number;
  if (rest === "min") value = -32767;
  else if (rest === "max") value = 32767;
  else {
    if (!/^-?\d+$/.test(rest)) return null;
    value = Number(rest);
    if (!Number.isInteger(value) || value < -32768 || value > 32767) return null;
    if (value === -32768) value = -32767;
  }
  if (value === 0) return null;
  return {
    mechanism: base === "lx" || base === "ly" ? "lstick" : "rstick",
    vertical: base === "ly" || base === "ry",
    positive: value > 0,
    exact: value === -32767 || value === 32767,
  };
}

/** Does `fn` point the same way as `want`, whatever it is spelled? `exact` is
 *  deliberately NOT compared: `ly.-16384` is the down half of the left stick
 *  just as `ly.min` is. Mirror of render_map.rs `points_same_way`. */
function pointsSameWay(fn: string, want: Pointing): boolean {
  const p = pointing(fn);
  return (
    p !== null &&
    p.mechanism === want.mechanism &&
    p.vertical === want.vertical &&
    p.positive === want.positive
  );
}

/** One PRESENTED control. Mirror of `ksx_core::diagonal::Held`. */
type Held =
  | {
      kind: "diag";
      diag: Diag;
      mechanisms: Mechanism[];
      /** INDICES into the original hold — the round trip is "put those strings
       *  back", so a hand-written `ly.-16384 + lx.max` displays as the diagonal
       *  and is stored byte for byte as it was written. */
      members: number[];
      exact: boolean;
    }
  | { kind: "plain"; member: number };

/** How to PRESENT this hold.
 *
 *  **Per mechanism bucket, "contains both" — never exact-set-equality on the
 *  whole step.** `down + forward + A` is the single most common macro step in
 *  existence (the attack that ends a motion) and it folds: `A` is a passenger.
 *  `down + forward + up` never folds — which diagonal would it be, and what the
 *  pad publishes depends on the slot's `socd` policy, resolved at plan time,
 *  which this page cannot see. */
function fold(hold: string[]): Held[] {
  const parsed = hold.map(pointing);
  const folded: { diag: Diag; mechanism: Mechanism; members: number[]; exact: boolean }[] = [];
  const consumed = hold.map(() => false);

  for (const mechanism of MECHANISMS) {
    const members: number[] = [];
    parsed.forEach((p, i) => {
      if (p !== null && p.mechanism === mechanism) members.push(i);
    });
    if (members.length === 0) continue;
    // POLARITIES are counted, not bindings: a hold naming `dpad.down` twice is
    // still one V−.
    let vertical: boolean | null = null;
    let horizontal: boolean | null = null;
    let split = false;
    let exact = true;
    for (const i of members) {
      const p = parsed[i]!;
      exact = exact && p.exact;
      if (p.vertical) {
        if (vertical === null) vertical = p.positive;
        else if (vertical !== p.positive) split = true;
      } else {
        if (horizontal === null) horizontal = p.positive;
        else if (horizontal !== p.positive) split = true;
      }
    }
    if (vertical === null || horizontal === null || split) continue;
    for (const i of members) consumed[i] = true;
    folded.push({ diag: diagFromHalves(vertical, horizontal), mechanism, members, exact });
  }

  // Coalesce: buckets that folded to the SAME diagonal are ONE presented
  // control. That is the hat+stick double-binding every in-box template writes —
  // one key on `dpad.down` AND `ly.min`.
  const out: Held[] = [];
  for (const diag of DIAGS) {
    const hits = folded.filter((f) => f.diag === diag);
    if (hits.length === 0) continue;
    const members = hits.flatMap((h) => h.members).sort((a, b) => a - b);
    out.push({
      kind: "diag",
      diag,
      mechanisms: hits.map((h) => h.mechanism),
      members,
      exact: hits.every((h) => h.exact),
    });
  }
  consumed.forEach((taken, i) => {
    if (!taken) out.push({ kind: "plain", member: i });
  });
  return out;
}

/** The cell token for a diagonal column. Contains a `:`, which no function name
 *  ever does, so a diagonal pick can never be mistaken for one — it is EXPANDED
 *  to the pair before anything is stored. */
function diagToken(m: Mechanism, d: Diag): string {
  return `diag:${mechanismToken(m)}:${d}`;
}

function parseDiagToken(token: string): { mechanism: Mechanism; diag: Diag } | null {
  if (!token.startsWith("diag:")) return null;
  const [, mech, diag] = token.split(":");
  const mechanism = MECHANISMS.find((m) => mechanismToken(m) === mech);
  if (!mechanism || !DIAGS.includes(diag as Diag)) return null;
  return { mechanism, diag: diag as Diag };
}

/** One position on the direction ring. */
type RingPos =
  | { kind: "card"; vertical: boolean; positive: boolean }
  | { kind: "diag"; diag: Diag };

/** **↑ ↖ ← ↙ ↓ ↘ → ↗** — numpad 8 7 4 1 2 3 6 9: walk the gate from up,
 *  counter-clockwise, around the bottom, back to up.
 *
 *  Why this order and not numpad-ascending or compass-clockwise: MOTIONS BECOME
 *  SHAPES. A piano roll is read as a picture, and this is the only ordering
 *  where the picture *is* the motion — a quarter-circle forward is a staircase
 *  sweeping right, a half-circle a straight 45° line, a dragon punch visibly a
 *  hook. Cardinals land on the even indices so each diagonal sits literally
 *  between its two parents. */
const RING: RingPos[] = [
  { kind: "card", vertical: true, positive: true },
  { kind: "diag", diag: "ul" },
  { kind: "card", vertical: false, positive: false },
  { kind: "diag", diag: "dl" },
  { kind: "card", vertical: true, positive: false },
  { kind: "diag", diag: "dr" },
  { kind: "card", vertical: false, positive: true },
  { kind: "diag", diag: "ur" },
];

interface MacroColumn {
  token: string;
  glyph: string;
  idcls: string;
  title: string;
  band: string;
}

/** Which band a control sits under. Mirror of render_map.rs `band_of`. */
function bandOf(fn: string): string {
  if (fn === "lt" || fn === "lb" || fn === "rb" || fn === "rt") return "SHOULDERS";
  if (fn === "A" || fn === "B" || fn === "X" || fn === "Y") return "FACE";
  if (fn === "guide" || fn === "back" || fn === "start") return "SYSTEM";
  if (fn === "lthumb") return mechanismBand("lstick");
  if (fn === "rthumb") return mechanismBand("rstick");
  const m = mechanismOf(fn);
  return m === null ? "SYSTEM" : mechanismBand(m);
}

/** The grid's columns for one persona: every non-direction control as itself,
 *  and every direction MECHANISM as its eight-position ring. 25 zones → 37
 *  columns. Mirror of render_map.rs `macro_columns`. */
function macroColumns(slot: MapperSlot | null): MacroColumn[] {
  const table = slot && isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const out: MacroColumn[] = [];
  const rung: Mechanism[] = [];
  for (const [fn, label] of table) {
    const mechanism = mechanismOf(fn);
    if (mechanism === null) {
      out.push({
        token: fn,
        glyph: label,
        idcls: "maccolid",
        title: `${legendLabel(fn, label)} (${fn})`,
        band: bandOf(fn),
      });
      continue;
    }
    // The mechanism's whole ring is emitted at its FIRST direction zone; the
    // other three zones of that mechanism are already in it.
    if (rung.includes(mechanism)) continue;
    rung.push(mechanism);
    for (const pos of RING) {
      if (pos.kind === "card") {
        const fnName = mechanismFunction(mechanism, pos.vertical, pos.positive);
        out.push({
          token: fnName,
          glyph: cardinalGlyph(pos.vertical, pos.positive),
          idcls: "maccolid card",
          title:
            `${mechanismGroup(mechanism)}${cardinalGlyph(pos.vertical, pos.positive)} · ` +
            `${cardinalWords(pos.vertical, pos.positive)} · ` +
            `numpad ${cardinalNumpad(pos.vertical, pos.positive)} · holds ${fnName}`,
          band: mechanismBand(mechanism),
        });
      } else {
        const [up, right] = DIAG_HALVES[pos.diag];
        out.push({
          token: diagToken(mechanism, pos.diag),
          glyph: DIAG_GLYPH[pos.diag],
          idcls: "maccolid diag",
          // `DIAG_MOVELIST` is FACING-RELATIVE and is labelled as such. ksx has
          // no notion of facing — it publishes a direction, not a side of the
          // screen — and it already ships the mirrored spelling of every motion
          // because player 2 is not an edge case. A bare "d/f" beside a compass
          // name reads as a second name for the same fact; it is a second name
          // for the fact HALF the time, and is d/b for the player on the right.
          title:
            `${mechanismGroup(mechanism)}${DIAG_GLYPH[pos.diag]} · ${DIAG_WORDS[pos.diag]} · ` +
            `numpad ${DIAG_NUMPAD[pos.diag]} · ${DIAG_MOVELIST[pos.diag]} in a move list, ` +
            `facing right · one pick, and ` +
            `ksx writes ${mechanismFunction(mechanism, true, up)} + ` +
            `${mechanismFunction(mechanism, false, right)}`,
          band: mechanismBand(mechanism),
        });
      }
    }
  }
  return out;
}

/** What a column is called in a sentence. Mirror of `column_name`. */
function columnName(slot: MapperSlot | null, column: MacroColumn): string {
  const d = parseDiagToken(column.token);
  if (d) return `${mechanismGroup(d.mechanism)}${DIAG_GLYPH[d.diag]} (${DIAG_WORDS[d.diag]})`;
  const m = mechanismOf(column.token);
  if (m !== null) return `${mechanismGroup(m)}${column.glyph} (${column.token})`;
  const table = slot && isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const def = table.find(([fn]) => fn.toLowerCase() === column.token.toLowerCase());
  return def ? `${legendLabel(def[0], def[1])} (${def[0]})` : column.token;
}

/** Every mechanism THIS SLOT's own bound direction keys drive, in the order a
 *  motion should prefer them. */
function drivenMechanisms(slot: MapperSlot | null): Mechanism[] {
  if (!slot) return [];
  const out: Mechanism[] = [];
  for (const [fn, keys] of Object.entries(slot.bindings ?? {})) {
    const live = (keys ?? []).filter((k) => k !== "" && k.toLowerCase() !== "none");
    if (live.length === 0) continue;
    const m = mechanismOf(fn);
    if (m !== null && !out.includes(m)) out.push(m);
  }
  return out;
}

/** The mechanism a generated motion should be written in: the one the player's
 *  own keys already drive, and the dpad when nothing says otherwise. */
function motionMechanism(slot: MapperSlot | null): Mechanism {
  return drivenMechanisms(slot)[0] ?? "dpad";
}

/** The four directional function names, on that mechanism.
 *
 *  ⚠ THE SIGN. "Up" on a stick is `ly.max`, NOT `ly.min` — XInput's positive Y
 *  is UP, and legacy binds Up to `AXIS_MAX`. Getting it backwards yields an ↖
 *  that looks right in every reader here and does nothing in the game, which is
 *  the worst failure this page can produce. There is exactly one place that
 *  decides it (`mechanismFunction`, which `socd::pointing` is pinned against),
 *  and this delegates to it rather than re-deriving the axis names. */
function directionFns(m: Mechanism): Record<Dir, string> {
  return {
    up: mechanismFunction(m, true, true),
    down: mechanismFunction(m, true, false),
    left: mechanismFunction(m, false, false),
    right: mechanismFunction(m, false, true),
  };
}

type Dir = "up" | "down" | "left" | "right";

/** The eight positions of the gate, as direction sets — clockwise from →.
 *  A full rotation is this list; every other motion is an arc of it. */
const GATE: Dir[][] = [
  ["right"],
  ["down", "right"],
  ["down"],
  ["down", "left"],
  ["left"],
  ["up", "left"],
  ["up"],
  ["up", "right"],
];

/** The same walk the other way round, from ←. */
const GATE_MIRRORED: Dir[][] = GATE.map((dirs) =>
  dirs.map((d) => (d === "left" ? "right" : d === "right" ? "left" : d)),
);

/** A motion as DIRECTIONS PER STEP — the shape of the thing, before it knows
 *  which mechanism will express it. The two-name entries are the whole lesson.
 *
 *  ALL FOUR DIAGONALS ARE HERE, because the motions are why they exist: a
 *  half-circle needs ↙ AND ↘, a dragon punch needs ↘ specifically, and a 360
 *  (the spinning piledriver) walks all eight positions of the gate — which is
 *  the motion that proves recognition and expansion agree, because every one of
 *  its four diagonal steps must read back as its own diagonal.
 *
 *  "Forward" is right, the way a move list writes it for a character on the
 *  left; the mirrored spellings are offered beside them because player 2 is not
 *  an edge case. */
const MOTIONS: Record<string, { label: string; steps: Dir[][] }> = {
  qcf: { label: "quarter-circle forward (↓ ↘ →)", steps: [["down"], ["down", "right"], ["right"]] },
  qcb: { label: "quarter-circle back (↓ ↙ ←)", steps: [["down"], ["down", "left"], ["left"]] },
  hcf: {
    label: "half-circle forward (← ↙ ↓ ↘ →)",
    steps: [["left"], ["down", "left"], ["down"], ["down", "right"], ["right"]],
  },
  hcb: {
    label: "half-circle back (→ ↘ ↓ ↙ ←)",
    steps: [["right"], ["down", "right"], ["down"], ["down", "left"], ["left"]],
  },
  dpf: { label: "dragon punch forward (→ ↓ ↘)", steps: [["right"], ["down"], ["down", "right"]] },
  dpb: { label: "dragon punch back (← ↓ ↙)", steps: [["left"], ["down"], ["down", "left"]] },
  // The full circle. Both facings, because a 360 is a rotation and which way
  // round it goes is the player's, not ours.
  spdf: { label: "360 forward (→ ↘ ↓ ↙ ← ↖ ↑ ↗)", steps: GATE },
  spdb: { label: "360 back (← ↙ ↓ ↘ → ↗ ↑ ↖)", steps: GATE_MIRRORED },
};

/** The sentence above the motion buttons: which mechanism they will write, and
 *  why that is the one. Empty when there is no slot to read. */
export function macroMotionLineFor(slot: MapperSlot | null): string {
  const driven = drivenMechanisms(slot);
  const pick = motionMechanism(slot);
  const tail =
    "Each one appends its steps to the macro below — the MIDDLE step of a quarter-circle is " +
    "the diagonal, and a 360 is four of them. You can tick any of them yourself in that " +
    "group's ↖ ↗ ↙ ↘ columns.";
  if (driven.length === 0) {
    return (
      `These write ${mechanismLabel(pick)} — this preset binds no direction keys of its own, ` +
      `so there is nothing to match. If the game reads a stick, retick the rows. ${tail}`
    );
  }
  if (driven.length > 1) {
    return (
      `These write ${mechanismLabel(pick)}. This preset's own direction keys drive ` +
      `${driven.map(mechanismLabel).join(" and ")}, so either would be read — a pad has three ` +
      `ways to say "right" and a game reads whichever one it was written for. ${tail}`
    );
  }
  return (
    `These write ${mechanismLabel(pick)} — the same mechanism this preset's own direction ` +
    `keys drive, so the game reads them. (A motion written on the other mechanism is ` +
    `published faithfully and read by nobody: that is the trap.) ${tail}`
  );
}

/** Append a ready-made motion to the draft. Returns what happened, in words,
 *  for the toast — or null when there is nothing to append it to. */
export function macroInsertMotion(name: string): string | null {
  const mac = macroDraft;
  const motion = MOTIONS[name];
  if (!mac || !motion) return null;
  const slot = currentSlot();
  const m = motionMechanism(slot);
  const fns = directionFns(m);
  const first = mac.steps.length;
  for (const dirs of motion.steps) {
    const step = newStep();
    step.hold = dirs.map((d) => fns[d]);
    mac.steps.push(step);
  }
  macroStep = first;
  macroEdited();
  const shape = motion.steps
    .map((dirs) => holdText(slot, dirs.map((d) => fns[d])))
    .join(" · ");
  // Every diagonal step, by its step number — a 360 has four of them, and the
  // point of saying so is that each one is ONE step, not two.
  const diagonals = motion.steps
    .map((dirs, i) => [dirs, i] as const)
    .filter(([dirs]) => dirs.length > 1);
  const which =
    diagonals.length === 1
      ? `Step ${first + diagonals[0][1] + 1} is the diagonal`
      : `Steps ${diagonals.map(([, i]) => first + i + 1).join(", ")} are the diagonals`;
  // FIX 3: the toast speaks the SHAPE, in diagonals, like the button that
  // produced it. What a diagonal is stored as is said once — on the row
  // itself, in `.macexp`, for every one of these steps — instead of being
  // repeated in every label and every toast until it reads as noise.
  return (
    `Added ${motion.steps.length} steps for the ${motion.label}, at 50 ms each, on ` +
    `${mechanismLabel(m)}: ${shape}. ${which} — ONE step each, and each one spells the ` +
    "pair it stores beside its name. You can tick the same cells yourself in that " +
    "group's ↖ ↗ ↙ ↘ columns. Add the attack button as a final step, then press Save macro."
  );
}

/** add / insert above / insert below / delete / move up / move down. */
export function macroStepVerb(verb: string, index: number): void {
  const mac = macroDraft;
  if (!mac) return;
  const n = mac.steps.length;
  switch (verb) {
    case "add":
      mac.steps.push(newStep());
      macroStep = mac.steps.length - 1;
      break;
    case "insa":
      if (index < 0 || index > n) return;
      mac.steps.splice(index, 0, newStep());
      macroStep = index;
      break;
    case "insb":
      if (index < 0 || index >= n) return;
      mac.steps.splice(index + 1, 0, newStep());
      macroStep = index + 1;
      break;
    case "del": {
      if (index < 0 || index >= n) return;
      // FIX 1 — THE DEAD END, made unreachable. Victor: "when I remove all the
      // steps by pressing x there is no way to add them back."
      //
      // `mapping::save_macro` REFUSES a zero-step table (empty steps is a
      // refusal, not a delete — deliberate, so a UI that loses its grid cannot
      // silently erase a macro). So zero steps is invalid by construction and
      // this editor must not be able to reach it: deleting the LAST remaining
      // step empties it instead of removing it. The author lands on a step
      // holding nothing — a legal, meaningful neutral gap — never on a blank
      // grid whose every add affordance lived on a row that no longer exists.
      if (n === 1) {
        mac.steps[0].hold = [];
        macroStep = 0;
        break;
      }
      mac.steps.splice(index, 1);
      macroStep = mac.steps.length === 0 ? null : Math.min(index, mac.steps.length - 1);
      break;
    }
    // FIX 2: the row's own unit toggle. Same conversion rule as the old panel
    // select — 50 ms picked as frames is 3 frames, not 50 of them.
    case "unit": {
      if (index < 0 || index >= n) return;
      macroStep = index;
      macroSetUnitAt(index, unitOfStep(mac.steps[index]) === "frames" ? "ms" : "frames");
      return;
    }
    case "up": {
      if (index <= 0 || index >= n) return;
      const [moved] = mac.steps.splice(index, 1);
      mac.steps.splice(index - 1, 0, moved);
      macroStep = index - 1;
      break;
    }
    case "down": {
      if (index < 0 || index >= n - 1) return;
      const [moved] = mac.steps.splice(index, 1);
      mac.steps.splice(index + 1, 0, moved);
      macroStep = index + 1;
      break;
    }
    case "sel":
      macroSelectStep(index);
      return;
    default:
      return;
  }
  macroEdited();
}

/** ONE ROW's duration, in the unit that row is authored in. `value <= 0` is
 *  ignored rather than written — a zero-length step is not a shorter step, it
 *  is a step the loader refuses.
 *
 *  FIX 2 — THE MODE IS GONE. This used to be "the SELECTED step's duration",
 *  which made changing a time a two-part gesture (pick the row, then find the
 *  one box under the grid) and made every poll that dropped the selection a
 *  silent way to lose the edit — the bug the pwtest suite's first three cases
 *  are about. Every row now carries its own box, so `index` is simply the row
 *  the author typed in. Selection still exists (the detail line and the frame
 *  maths have to be ABOUT something), but it now FOLLOWS the edit instead of
 *  gating it. */
export function macroSetDurationAt(index: number, value: number): void {
  const step = macroDraft?.steps[index];
  if (!step || !Number.isFinite(value) || value <= 0) return;
  const n = Math.round(value);
  const want = unitOfStep(step);
  // A number typed into a row's box is authored in whatever unit that row is
  // showing, so the write records BOTH halves — value and unit.
  stepUnits.set(step, want);
  macroLastUnit = want;
  macroStep = index;
  if (want === "frames") {
    step.frames = n;
    step.ms = null;
  } else {
    step.ms = n;
    step.frames = null;
  }
  macroEdited();
}

/** Switch ONE ROW between `ms` and `frames`, CONVERTING rather than
 *  reinterpreting: 50 ms picked as frames is 3 frames, not 50 of them. The
 *  unit is an authoring convenience (§1c — it buys readability and nothing
 *  else), so changing it must not change how long the step runs. */
function macroSetUnitAt(index: number, want: StepUnit): void {
  // Remembered even when it lands on nothing, so a fresh step starts in the
  // unit the author has been working in.
  macroLastUnit = want;
  const step = macroDraft?.steps[index];
  if (!step) {
    refreshMacro();
    return;
  }
  const already = stepUnits.get(step) === want && fileUnitOf(step) === want;
  stepUnits.set(step, want);
  if (want === "frames") {
    if (step.frames === null) {
      step.frames = Math.max(1, Math.round(((step.ms ?? 50) * 60) / 1000));
      step.ms = null;
    }
  } else if (step.ms === null) {
    step.ms = framesMs(step.frames ?? 1);
    step.frames = null;
  }
  // Picking the unit a step is already in is not an edit — it must not light
  // up "unsaved changes" over a choice that changed nothing.
  if (already) {
    refreshMacro();
    return;
  }
  macroEdited();
}

/** What map.ts writes into row `index`'s duration box after every edit and
 *  every poll (a dirty form control ignores its attribute, so the value cannot
 *  come from the markup). Empty string for a row that is not there. */
export function macroRowDuration(index: number): string {
  const step = macroDraft?.steps[index];
  if (!step) return "";
  return String(unitOfStep(step) === "frames" ? (step.frames ?? 1) : (step.ms ?? 50));
}

export function macroSetAllowShort(on: boolean): void {
  const step = macroStep === null ? undefined : macroDraft?.steps[macroStep];
  if (!step) return;
  step.allow_short = on;
  macroEdited();
}

/** One of the three macro-level policies. Unknown words are refused here, not
 *  written into a block that would then fail to load. */
export function macroSetPolicy(field: string, value: string): void {
  const mac = macroDraft;
  if (!mac) return;
  if (field === "on_release" && (value === "finish" || value === "abort")) {
    mac.on_release = value;
  } else if (field === "retrigger" && (value === "ignore" || value === "restart")) {
    mac.retrigger = value;
  } else if (
    field === "interrupt" &&
    (value === "none" || value === "any-input" || value === "opposing")
  ) {
    mac.interrupt = value;
  } else if (
    field === "repeat" &&
    (value === "once" || value === "while-held" || value === "turbo")
  ) {
    mac.repeat = value;
    // Turning turbo ON with no rate would write a table the loader refuses
    // ("is `repeat = \"turbo\"` but gives no rate"), so the editor seeds one
    // that is actually deliverable rather than letting Save be the way the
    // author finds out. Turning it OFF keeps the number: flipping the policy
    // back and forth must not lose it, which is the file format's own rule.
    if (value === "turbo" && mac.turbo_hz === null && mac.gap_ms === null) {
      mac.turbo_hz = 10;
    }
  } else {
    return;
  }
  macroEdited();
}

/** The turbo RATE, in the unit the box is currently showing.
 *
 *  Exactly one of `turbo_hz`/`gap_ms` survives, always: they are two spellings
 *  of one number, and a table that gives both is refused — so switching the
 *  unit MOVES the value rather than adding a second field. A blank box clears
 *  the rate entirely (which validation then names, if the policy is turbo). */
export function macroSetTurboRate(value: string, unit: string): void {
  const mac = macroDraft;
  if (!mac) return;
  const text = value.trim();
  if (text === "") {
    mac.turbo_hz = null;
    mac.gap_ms = null;
    macroEdited();
    return;
  }
  const n = Number(text);
  if (!Number.isFinite(n) || n < 0) return;
  const rounded = Math.round(n);
  if (unit === "gap_ms") {
    mac.turbo_hz = null;
    mac.gap_ms = rounded;
  } else {
    mac.turbo_hz = rounded;
    mac.gap_ms = null;
  }
  macroEdited();
}

/** The rate box's value, for map.ts (a value attribute cannot be written by a
 *  binding once the user has typed into the box). */
export function macroTurboBoxValue(): string {
  const mac = macroDraft;
  if (!mac) return "";
  if (mac.turbo_hz !== null) return String(mac.turbo_hz);
  if (mac.gap_ms !== null) return String(mac.gap_ms);
  return "";
}

/** Which unit the rate box is showing — map.ts writes it onto the <select>,
 *  which an attribute binding cannot do (same seam as `macroStepUnit`). */
export function macroRateUnit(): string {
  return macroDraft?.gap_ms !== null && macroDraft?.gap_ms !== undefined
    ? "gap_ms"
    : "turbo_hz";
}

/** The repeat policy the selects should show. */
export function macroRepeatValue(): string {
  return macroDraft?.repeat || "once";
}

/** The draft's TRIGGER keys, for a rename that must not lose them. */
export function macroDraftTriggers(): string[] {
  return macroDraft ? [...macroDraft.triggers] : [];
}

export function macroTomlText(): string {
  return macroDraft === null ? "" : macroTomlFor(macroDraft);
}

// ── Derivations (mirror render_map.rs) ─────────────────────────────────────

/** The glyph row: ARROWS ONLY, one line, no digits — it is a screen, and
 *  screens speak arrows in this genre. The numpad digit lives in the tooltip
 *  and the ring line, where it is a lookup key rather than a label. */
function macroColsFor(slot: MapperSlot | null): MacroCol[] {
  return macroColumns(slot).map((c) => ({
    fn: c.token,
    id: c.glyph,
    // UNIFORM colour, deliberately: a header row of coloured discs at column
    // width is noise rather than information. The identity colours earn their
    // place on the controller art (where they map to physical buttons) and in
    // the legend beside it — here the column is NAMED, not badged.
    // `card`/`diag` are TYPE, not palette.
    idcls: c.idcls,
    title: c.title,
  }));
}

/** The GROUP BAND above the glyph row — one cell per run of columns belonging
 *  to the same part of the pad, spanning it. */
function macroGroupsFor(slot: MapperSlot | null): MacroGroup[] {
  const runs: { label: string; span: number }[] = [];
  for (const column of macroColumns(slot)) {
    const last = runs[runs.length - 1];
    if (last && last.label === column.band) last.span += 1;
    else runs.push({ label: column.band, span: 1 });
  }
  // A CLASS, never an inline `grid-column`: forma's CSP nonce-locks style-src
  // (ledger #13), and the span is a closed set anyway.
  return runs.map((r) => ({ label: r.label, cls: `macgrp g${r.span}` }));
}

/** How one PRESENTED control is named on this pad.
 *
 *  A coalesced diagonal (the hat+stick double-binding every in-box template
 *  writes) joins its mechanisms with **"and"**, never with `+`. `+` is this
 *  row's separator for ANOTHER CONTROL — "D-pad ↘ + A" is two things held at
 *  once — so `"D-pad + LS ↘"` read as a control called "D-pad" and a control
 *  called "LS ↘", which is two lies in four words: the D-pad is pointing ↘ too,
 *  and there is only one control here. It also collided with the `· together`
 *  tail, which that row does not get (it folds to one presented control), so
 *  the row that holds the MOST bindings looked like the row that holds two.
 *  "D-pad and LS ↘" is one control, said on two mechanisms — the same joiner
 *  `macroMotionLine` already uses for exactly this list. */
function heldLabel(slot: MapperSlot | null, hold: string[], held: Held): string {
  if (held.kind === "diag") {
    return `${held.mechanisms.map((m) => mechanismGroup(m).trim()).join(" and ")} ${DIAG_GLYPH[held.diag]}`;
  }
  const table = slot && isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const f = hold[held.member];
  const def = table.find(([fn]) => fn.toLowerCase() === f.toLowerCase());
  return def ? legendLabel(def[0], def[1]) : f;
}

/** WHAT THIS ROW HOLDS, in words, beside the row — "D-pad ↘ + A".
 *
 *  This is where the model is TAUGHT. Victor lost an evening to a "hadouken"
 *  whose rows were ↓ then → then X, because a diagonal is not a thing you bind —
 *  it IS down+forward held at once. Now it IS a thing you bind, in the piano;
 *  the row reads it back as one control (which is what the player means) and
 *  `holdExpand` says beside it exactly which two names the file carries.
 *
 *  Named the way THIS pad names it (so it matches the column headers and the
 *  art above), and `+` between them because that is the notation every player
 *  already reads. */
function holdText(slot: MapperSlot | null, hold: string[]): string {
  if (hold.length === 0) return "(nothing — neutral gap)";
  return fold(hold)
    .map((h) => heldLabel(slot, hold, h))
    .join(" + ");
}

/** THE LEDGER LINE: every diagonal on this row spelled as the pair the file
 *  stores — `↘ = dpad.down + dpad.right`. The presentation says "one control",
 *  the storage says "two holds", and this says both at once so nobody has to
 *  open the TOML to find out what a pick wrote. */
function holdExpand(hold: string[]): string {
  return fold(hold)
    .filter((h): h is Extract<Held, { kind: "diag" }> => h.kind === "diag")
    .map((h) => `${DIAG_GLYPH[h.diag]} = ${h.members.map((i) => hold[i]).join(" + ")}`)
    .join(" · ");
}

/** The readout's own class, over PRESENTED controls rather than stored ones: a
 *  diagonal is ONE control, so `↓ + →` no longer reads as two. The accent is
 *  for a row that really does hold several things at once. */
function holdCls(hold: string[]): string {
  const n = fold(hold).length;
  if (n === 0) return "machold none";
  return n > 1 ? "machold both" : "machold";
}

/** The row's hover text — what it holds, how long, and the stored pair when a
 *  diagonal is on it. Mirror of render_map.rs `row_title`, which carries the
 *  reasoning. */
function rowTitle(slot: MapperSlot | null, step: MacroStepView, i: number): string {
  const base =
    `step ${i + 1} holds ${holdText(slot, step.hold)} for ${durationText(step)} ` +
    `(the engine runs it for ${effectiveMs(step)} ms)`;
  const expand = holdExpand(step.hold);
  return expand === "" ? base : `${base} — ${expand}`;
}

function macroRowsFor(mac: MacroView, slot: MapperSlot | null): MacroRow[] {
  const last = mac.steps.length - 1;
  const only = mac.steps.length === 1;
  return mac.steps.map((step, i) => {
    const warn = stepWarning(step);
    const unit = unitOfStep(step);
    return {
      n: String(i + 1),
      cls: `macrow${warn === "" ? "" : " short"}${macroStep === i ? " sel" : ""}`,
      dur: durationText(step),
      // FIX 2: the number, in the unit this step was authored in, in this
      // row's own box. The box's VALUE is written by map.ts after every edit
      // and every poll (a dirty form control ignores its attribute), so it is
      // the row index that travels in the markup, not the number.
      durval: String(unit === "frames" ? (step.frames ?? 1) : (step.ms ?? 50)),
      durrow: String(i),
      durcls: stepIsShort(step) ? "macrowdur short" : "macrowdur",
      unit: unit === "frames" ? "fr" : "ms",
      unitact: `unit|${i}`,
      unittitle:
        unit === "frames"
          ? `step ${i + 1} is authored in FRAMES — click to switch it to ms (the length is ` +
            "converted, never reinterpreted)"
          : `step ${i + 1} is authored in MILLISECONDS — click to switch it to frames (the ` +
            "length is converted, never reinterpreted; ksx counts frames at 60 Hz)",
      // The ledger is said TWICE — see render_map.rs `row_title`. The `.macexp`
      // span beside the words is the primary place, but it is the first thing
      // the bar gives up as the card narrows and it gives it up silently (154px
      // at a 1440 viewport, 60px at 1100, ZERO at 820, `display: block`
      // throughout). The row title cannot be truncated, so it carries the pair
      // as well and the narrow-width drop becomes a layout choice instead of
      // the quiet loss of the only statement of what a pick wrote.
      durtitle: rowTitle(slot, step, i),
      hold: holdText(slot, step.hold),
      holdcls: holdCls(step.hold),
      exp: holdExpand(step.hold),
      expcls: holdExpand(step.hold) === "" ? "macexp off" : "macexp",
      warn,
      warntitle: stepWarningLong(step),
      warncls: warn === "" ? "macwarn off" : "macwarn",
      selact: `sel|${i}`,
      upact: `up|${i}`,
      dnact: `down|${i}`,
      iaact: `insa|${i}`,
      ibact: `insb|${i}`,
      delact: `del|${i}`,
      // FIX 1: on the last remaining step this button EMPTIES the row rather
      // than removing it — and says so, so it does not read as a broken
      // delete. A macro with no steps is refused by the writer, so the editor
      // is not allowed to build one.
      deltitle: only
        ? "clear this step (a macro needs at least one — this empties the row instead of " +
          "removing it, which is a legal neutral gap)"
        : "delete this step",
      upcls: i === 0 ? "macbtn off" : "macbtn",
      dncls: i === last ? "macbtn off" : "macbtn",
    };
  });
}

/** The matrix, FLAT: `steps × 37` cells in row-major order.
 *
 *  A step holding `ly.min` + `lx.max` lights the LS `↘` cell — not two stray
 *  ticks eight columns apart. That is the whole point, and it works on a
 *  hand-written step nobody made through this page. The two cardinals still
 *  show a subordinate tick (`part`), because pretending they are not in the
 *  file would be the lens lying about the storage. */
function macroCellsFor(mac: MacroView, slot: MapperSlot | null): MacroCell[] {
  const columns = macroColumns(slot);
  const cells: MacroCell[] = [];
  mac.steps.forEach((step, i) => {
    const view = fold(step.hold);
    const memberOf = new Map<number, Diag>();
    const lit: { mechanism: Mechanism; diag: Diag; exact: boolean }[] = [];
    for (const held of view) {
      if (held.kind !== "diag") continue;
      for (const m of held.members) memberOf.set(m, held.diag);
      for (const mechanism of held.mechanisms) {
        lit.push({ mechanism, diag: held.diag, exact: held.exact });
      }
    }
    for (const column of columns) {
      const d = parseDiagToken(column.token);
      let state: "off" | "on" | "part" = "off";
      let partOf: Diag | null = null;
      let approx = false;
      if (d) {
        const hit = lit.find((l) => l.mechanism === d.mechanism && l.diag === d.diag);
        if (hit) {
          state = "on";
          approx = !hit.exact;
        }
      } else {
        const want = pointing(column.token);
        if (want) {
          // Matched by WHERE IT POINTS, not by spelling: `ly.-16384` is the
          // down half of this pad's left stick however the file spells it.
          const at = step.hold.findIndex((f) => pointsSameWay(f, want));
          if (at >= 0) {
            const diag = memberOf.get(at);
            if (diag === undefined) state = "on";
            else {
              state = "part";
              partOf = diag;
            }
          }
        } else if (step.hold.some((f) => f.toLowerCase() === column.token.toLowerCase())) {
          state = "on";
        }
      }
      const name = columnName(slot, column);
      let title: string;
      if (state === "on" && approx) {
        title = `step ${i + 1} holds ${name} — as written, not at full deflection (${step.hold.join(" + ")})`;
      } else if (state === "on") {
        title = `step ${i + 1} holds ${name}`;
      } else if (state === "part" && partOf !== null) {
        const g = DIAG_GLYPH[partOf];
        title = `step ${i + 1} holds ${name} as half of ${g} — the ${g} column beside it is the pick`;
      } else {
        title = `step ${i + 1} does not hold ${name}`;
      }
      cells.push({
        cls:
          "maccell" +
          (state === "on" ? " on" : state === "part" ? " part" : "") +
          (approx ? " approx" : "") +
          (d ? " isdiag" : "") +
          (macroStep === i ? " inrow" : ""),
        cell: `${i}|${column.token}`,
        mark: state === "on" ? "●" : state === "part" ? "·" : "",
        title,
      });
    }
  });
  return cells;
}

function macroTabsFor(p: MapPayload, mac: MacroView, slotNumber: number): MacroTab[] {
  return p.macros.macros.map((m) => ({
    name: m.name,
    label: `${m.name} · ${m.steps.length} steps`,
    href: `/map?slot=${slotNumber}&macro=${encodeURIComponent(m.name)}`,
    cls: m.name.toLowerCase() === mac.name.toLowerCase() ? "mactab active" : "mactab",
  }));
}

/** The same strip with nothing selected — the preset's macros are still all
 *  there to click, which is the way back into the editor. */
function macroTabsForNone(p: MapPayload, slotNumber: number): MacroTab[] {
  return p.macros.macros.map((m) => ({
    name: m.name,
    label: `${m.name} · ${m.steps.length} steps`,
    href: `/map?slot=${slotNumber}&macro=${encodeURIComponent(m.name)}`,
    cls: "mactab",
  }));
}

function tomlStr(text: string): string {
  return `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n")}"`;
}

/** The block you paste — `ksx_config::MacroFile`'s own spelling, defaults
 *  omitted, the duration in the unit it was authored in, and the trigger row
 *  underneath (COMMENTED when there is none, because a pasted
 *  `macro.x = "<KEY>"` would not load). */
function macroTomlFor(mac: MacroView): string {
  let out = `[macros.${mac.name}]\n`;
  if (mac.on_release !== "finish") out += `on_release = ${tomlStr(mac.on_release)}\n`;
  if (mac.retrigger !== "ignore") out += `retrigger = ${tomlStr(mac.retrigger)}\n`;
  if (mac.interrupt !== "none") out += `interrupt = ${tomlStr(mac.interrupt)}\n`;
  if (mac.repeat !== "" && mac.repeat !== "once") out += `repeat = ${tomlStr(mac.repeat)}\n`;
  // Two spellings of one number, so exactly ONE is emitted: a block giving
  // both is refused by the loader, and pasting one back must never be how a
  // reader finds that out.
  if (mac.turbo_hz !== null) out += `turbo_hz = ${mac.turbo_hz}\n`;
  else if (mac.gap_ms !== null) out += `gap_ms = ${mac.gap_ms}\n`;
  out += "steps = [\n";
  for (const step of mac.steps) {
    const hold = step.hold.map(tomlStr).join(", ");
    let duration: string;
    if (step.ms !== null && step.frames !== null) {
      duration = `ms = ${step.ms}, frames = ${step.frames}`;
    } else if (step.ms !== null) {
      duration = `ms = ${step.ms}`;
    } else if (step.frames !== null) {
      duration = `frames = ${step.frames}`;
    } else {
      duration = "ms = ";
    }
    out += `  { hold = [${hold}], ${duration}${step.allow_short ? ", allow_short = true" : ""} },\n`;
  }
  out += "]\n\n[bindings]\n";
  if (mac.triggers.length === 0) {
    out +=
      `# macro.${mac.name} = "<KEY>"   # no trigger yet — bind one above, ` +
      "or with the line below\n";
  } else if (mac.triggers.length === 1) {
    out += `macro.${mac.name} = ${tomlStr(mac.triggers[0])}\n`;
  } else {
    out += `macro.${mac.name} = [${mac.triggers.map(tomlStr).join(", ")}]\n`;
  }
  return out;
}

/** The slot switch, in words — and the exact line to change. Empty when the
 *  slot runs macros, which is the ordinary case. */
export function slotMacrosLineFor(slot: MapperSlot | undefined): string {
  if (!slot?.macros_off) return "";
  return (
    `Slot ${slot.number} says macros = "off" — the TOURNAMENT SWITCH. Nothing in this ` +
    `card runs on it, whatever each macro's own switch says, and nothing is deleted. To ` +
    `bring them back, set macros = "on" on that [[slot]] in config.toml (or on the slot of ` +
    `the games.toml profile you are running) and reload the session.`
  );
}

function macroTriggerLineFor(mac: MacroView): string {
  if (mac.triggers.length === 0) return "no trigger key yet — nothing starts this macro";
  if (mac.triggers.length === 1) return `started by ${mac.triggers[0]}`;
  return `started by ${mac.triggers.join(KEY_SEP)} — any one of them (${mac.triggers.length} keys)`;
}

function macroNoteFor(p: MapPayload | null, mac: MacroView | null): string {
  if (!p || !p.macros.available) {
    return (
      `This preset's macros could not be read (${p?.macros.reason ?? "no snapshot yet"}), so ` +
      "there is nothing to edit and nothing here can be saved. That is NOT the same as " +
      "\"this preset has no macros\" — it means nobody could tell this page either way."
    );
  }
  if (p.macros.macros.length === 0) {
    return (
      "This preset has no macros yet. Type a name above and press ＋ New macro: it is " +
      "written into the preset straight away (one empty 50 ms step), and then you paint the " +
      "grid and press Save macro."
    );
  }
  if (!mac) {
    return "Pick a macro above to edit it, or type a name and press ＋ New macro.";
  }
  return (
    `Steps and policies are a DRAFT until you press Save macro — that writes the whole ` +
    `"${mac.name}" table into the preset file (a timestamped backup is taken first) and ` +
    "swaps it into a running session with the pads left plugged. New, Rename and Delete " +
    "write immediately. Every one of them can be undone from the toast it leaves."
  );
}

/** One place where the whole macro card reaches the screen. */
function refreshMacro(): void {
  const p = lastPayload;
  const slot = currentSlot();
  const mac = macroDraft;
  const preset = p && p.macros.preset !== "" ? p.macros.preset : (slot?.preset ?? "<PRESET>");
  if (!mac) {
    // No macro loaded: the card stays, every reader says so, and the only
    // affordance that does anything is "＋ New macro". Nothing invents a
    // sequence the preset does not hold.
    setMacroTabs(p ? macroTabsForNone(p, slot ? slot.number : p.selected) : []);
    setMacroCols(macroColsFor(slot));
    setMacroGroups(macroGroupsFor(slot));
    setMacroRows([]);
    setMacroCells([]);
    setMacroGridCls("macgrid empty");
    setMacroNote(macroNoteFor(p, null));
    setMacroHead(
      p && p.macros.available && p.macros.macros.length === 0
        ? `"${preset}" has no macros yet`
        : "no macro selected",
    );
    setMacroRuleLine(MACRO_RULE_LINE);
    setMacroRingLine(MACRO_RING_LINE);
    setMacroPolicyLine("");
    setMacroTurboLine("");
    setMacroTurboValue("");
    setMacroTriggerLine("");
    setMacroFnName("");
    setMacroName("");
    setMacroCliLine(`ksx map --preset "${preset}" --function macro.<NAME> --key <KEY>`);
    setMacroToml("");
    setMacroCardCls(p?.macros.available ? "card macrocard" : "card macrocard off");
    setMacroDirtyLine("");
    setMacroSaveCls("btn btn-mini macsave off");
    setMacroEnableCls("btn btn-mini macen off dead");
    setMacroEnableLabel("Enabled");
    setSlotMacrosLine(slotMacrosLineFor(slot));
    setMacroStepLine("");
    setMacroConfirmCls("macconfirm off");
    setMacroConfirmLine("");
    setMacroMotionLine(macroMotionLineFor(slot));
    setMacroMathLine(frameMath(undefined, macroRateHz));
    setMacroTrigCls("mactrigger off");
    return;
  }
  setMacroCols(macroColsFor(slot));
  setMacroGroups(macroGroupsFor(slot));
  setMacroRows(macroRowsFor(mac, slot));
  setMacroCells(macroCellsFor(mac, slot));
  setMacroTabs(p ? macroTabsFor(p, mac, slot ? slot.number : p.selected) : []);
  setMacroHead(
    `${mac.name} — ${mac.steps.length} step${mac.steps.length === 1 ? "" : "s"} · ` +
      `${macroTotalMs(mac)} ms total` +
      // Loud, and in the head line, because everything below it describes
      // something that will not happen.
      (mac.disabled ? " · DISABLED (keeps its steps and its trigger; never runs)" : ""),
  );
  setMacroRuleLine(MACRO_RULE_LINE);
  setMacroRingLine(MACRO_RING_LINE);
  setMacroPolicyLine(
    `on release: ${mac.on_release} · retrigger: ${mac.retrigger} · ` +
      `interrupt: ${mac.interrupt} · repeat: ${mac.repeat || "once"}` +
      (mac.turbo_hz !== null
        ? ` (${mac.turbo_hz} Hz)`
        : mac.gap_ms !== null
          ? ` (${mac.gap_ms} ms gap)`
          : ""),
  );
  setMacroTurboLine(turboMath(mac));
  setMacroTurboValue(
    mac.turbo_hz !== null
      ? String(mac.turbo_hz)
      : mac.gap_ms !== null
        ? String(mac.gap_ms)
        : "",
  );
  setMacroNote(macroNoteFor(p, mac));
  setMacroTriggerLine(macroTriggerLineFor(mac));
  setMacroFnName(`macro.${mac.name}`);
  setMacroName(mac.name);
  setMacroCliLine(
    `ksx map --preset "${preset}" --function macro.${mac.name} --key ` +
      `${mac.triggers[0] ?? "<KEY>"}`,
  );
  setMacroToml(macroTomlFor(mac));
  setMacroCardCls(p?.macros.available ? "card macrocard" : "card macrocard off");
  setMacroGridCls(mac.steps.length === 0 ? "macgrid empty" : "macgrid");
  setMacroDirtyLine(
    macroDirty ? "unsaved changes — press Save macro to write them to the preset" : "saved",
  );
  setMacroSaveCls(macroDirty ? "btn btn-mini macsave dirty" : "btn btn-mini macsave off");
  // The switch reads as the STATE it is in, not as the action it performs: a
  // button labelled "Disable" on a macro that is already off is the one thing
  // a person in a hurry cannot read correctly.
  setMacroEnableCls(mac.disabled ? "btn btn-mini macen offstate" : "btn btn-mini macen on");
  setMacroEnableLabel(mac.disabled ? "DISABLED — click to enable" : "Enabled");
  setSlotMacrosLine(slotMacrosLineFor(slot));
  setMacroTrigCls(macroFromDisk ? "mactrigger" : "mactrigger off");
  const step = macroStep === null ? undefined : mac.steps[macroStep];
  setMacroStepLine(
    step === undefined
      ? "every step's time is its own box on its own row — type in the row you want"
      : `step ${(macroStep ?? 0) + 1} of ${mac.steps.length} — ${durationText(step)}` +
        (stepWarningLong(step) === "" ? "" : ` · ${stepWarningLong(step)}`),
  );
  setMacroMotionLine(macroMotionLineFor(slot));
  // The question is only ever on screen because Save asked it — and it is
  // re-derived here so it cannot outlive the steps it is about.
  const question = macroSaveAsked ? shortStepQuestion(mac) : "";
  setMacroConfirmLine(question);
  setMacroConfirmCls(question === "" ? "macconfirm off" : "macconfirm");
  if (question === "") macroSaveAsked = false;
  setMacroMathLine(frameMath(step, macroRateHz));
}

// ── FIX 2: Save's inline confirmation ──────────────────────────────────────
// The rule under the grid ("amber steps are shorter than…") has been there
// since v11 and was read as decoration by the one person it was written for —
// who then authored 1-frame steps and spent an evening on a macro the sampler
// could not deliver. So the consequence now stands between the click and the
// write, once, in the same words: a question with the count in it.
//
// It never REFUSES. A short step is legal, `allow_short` exists so one can be
// authored on purpose, and Studio does not overrule a file the loader accepts.
// The second click is the whole ceremony.

/** ASK: put the question on screen and answer "yes, I asked". Returns false
 *  when there is nothing to ask about, which is the ordinary save. */
export function macroAskAboutShortSteps(): boolean {
  const mac = macroDraft;
  if (!mac) return false;
  const question = shortStepQuestion(mac);
  if (question === "") return false;
  macroSaveAsked = true;
  refreshMacro();
  return true;
}

/** The question is answered (either way) — take it down. */
export function macroClearShortStepQuestion(): void {
  if (!macroSaveAsked) return;
  macroSaveAsked = false;
  refreshMacro();
}

/** What the bar is asking right now — "" when it is not up. For map.ts's
 *  toast, so the two cannot word the same fact differently. */
export function macroShortStepQuestion(): string {
  return macroDraft === null ? "" : shortStepQuestion(macroDraft);
}

/** The unit the FOCUSED step is authored in — what the detail line under the
 *  grid is talking about. Read from [`stepUnits`] (the authored choice) and
 *  NEVER re-derived from the value; with nothing focused it answers the last
 *  unit the author picked. Each ROW's own unit is [`unitOfStep`], which is what
 *  the row's toggle reads and writes since FIX 2. */
export function macroStepUnit(): StepUnit {
  const step = macroStep === null ? undefined : macroDraft?.steps[macroStep];
  if (!step) return macroLastUnit;
  return unitOfStep(step);
}

export function macroStepAllowShort(): boolean {
  const step = macroStep === null ? undefined : macroDraft?.steps[macroStep];
  return step?.allow_short ?? false;
}

// ── The screen ─────────────────────────────────────────────────────────────
// Nineteen createShow pairs. Their DOCUMENT ORDER no longer matters
// (2026-08-06, ledger #4 closed): compiler 0.3.1 names each show slot after
// its condition getter — `createShow(() => canLearn(), …)` becomes
// `show:canLearn` — and render_map.rs injects by that name, so a show can be
// added, removed or moved WHERE IT BELONGS VISUALLY without renumbering
// anything. `selBar` is still authored last only because that is where a
// `position: fixed` bar reads naturally; it is no longer a rule.
// `embedded_map_ir_slot_layout_matches_the_seam` asserts the NAME SET on both
// sides, so a rename here fails loudly instead of showing the wrong panel.

export function MapIsland() {
  return h(
    "div",
    { class: "studio mapper" },
    h(
      "header",
      { class: "top" },
      h(
        "div",
        { class: "brand" },
        h("span", { class: "brand-ksx" }, "ksx"),
        h("span", { class: "brand-studio" }, "Studio"),
      ),
      h(
        "nav",
        { class: "topnav", "aria-label": "screens" },
        h("a", { class: "navlink", href: "/" }, "Status"),
        h("a", { class: "navlink on", href: "/map", "aria-current": "page" }, "Mapper"),
      ),
      createShow(
        () => pillRunning(),
        () => h("span", { class: "pill pill-run" }, "running"),
      ),
      createShow(
        () => pillIdle(),
        () => h("span", { class: "pill pill-idle" }, "idle"),
      ),
      createShow(
        () => pillDown(),
        () => h("span", { class: "pill pill-down" }, "no daemon"),
      ),
      createShow(
        () => pillPaused(),
        () => h("span", { class: "pill pill-paused" }, "paused for mapping"),
      ),
    ),
    h(
      "main",
      null,
      // ── FIX 1: the no-daemon banner. TOP of the page, not buried at the
      // bottom of a card — the failure it exists for is a page that looks
      // completely normal and silently ignores every click. ──────────────
      createShow(
        () => noDaemon(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            h(
              "h2",
              null,
              "No daemon — ksx Studio can see your config but cannot change anything.",
            ),
            h(
              "p",
              { class: "alarmlead" },
              "Bindings below are the real ones on disk. Nothing you click here can ",
              "be saved until a daemon is running. Two ways to start one:",
            ),
            h(
              "ol",
              { class: "alarmways" },
              h("li", null, "the ksx tray icon → Start emulation, or"),
              h(
                "li",
                null,
                "run this in a shell: ",
                h("code", { class: "mono copyable" }, () => daemonCmd()),
              ),
            ),
          ),
      ),
      // ── FIX 0: emulation is running, so the learner cannot hear the panel.
      // One click to obey that instead of a dead end. ────────────────────
      createShow(
        () => sessionRunning(),
        () =>
          h(
            "section",
            { class: "card alarm warn" },
            h(
              "h2",
              null,
              "Emulation is running: panel keys are captured, so ksx can't hear them ",
              "for mapping.",
            ),
            h(
              "p",
              { class: "alarmlead" },
              "Pausing unplugs the pads and gives the panel back to Windows; ",
              "Resume starts the same profile again when you are done.",
            ),
            h(
              "div",
              { class: "pactrow" },
              // v9: a real form too, so this is never a dead button on a
              // page without JavaScript (the same ControlSource `stop` verb
              // the status page's form uses, 303'd back to /map).
              h(
                "form",
                { class: "pactform", method: "post", action: "/map/session/stop" },
                h(
                  "button",
                  { class: "btn btn-primary", "data-act": "pause-map", type: "submit" },
                  "Pause emulation & map",
                ),
              ),
            ),
          ),
      ),
      // ── FIX 0: the road back, persistent while this page holds the pause.
      createShow(
        () => pausedBar(),
        () =>
          h(
            "section",
            { class: "card alarm paused" },
            h("h2", null, "Emulation is paused for mapping."),
            h(
              "p",
              { class: "alarmlead" },
              "The cabinet has no virtual pads right now. Map what you need, then ",
              "put it back:",
            ),
            h(
              "div",
              { class: "pactrow" },
              h(
                "button",
                { class: "btn btn-primary", "data-act": "resume", type: "button" },
                "Resume emulation",
              ),
            ),
          ),
      ),
      // ── Slot context strip ────────────────────────────────────────────
      // ── The slot rail ────────────────────────────────────────────────
      // v14: was a card of pills above two lines of text, which read as the
      // page's first CONTENT. It is navigation — which player am I editing —
      // so it sits in a sticky bar with the identity of the current slot
      // beside it, and nothing below it moves when you switch.
      h(
        "section",
        { class: "slotstrip" },
        h(
          "div",
          { class: "tabs", role: "tablist", "aria-label": "slot" },
          createList(
            () => slotTabs(),
            (t) => t.num + "|" + t.label + "|" + t.cls,
            // v9: an ANCHOR, not a button. `/map?slot=N` is a route the
            // server has always understood, so switching slots is one GET
            // with JavaScript off; map.ts intercepts the click and switches
            // in place (no navigation, no lost scroll position) with it on.
            (t) => h("a", { class: t.cls, href: t.href, "data-slot": t.num }, t.label),
          ),
        ),
        h(
          "div",
          { class: "slotmeta" },
          h("p", { class: "slotline" }, () => slotLine()),
          h("p", { class: "srcline mono" }, () => sourceLine()),
        ),
      ),
      // ── Read-only banner + CLI fallback ───────────────────────────────
      createShow(
        () => readOnly(),
        () =>
          h(
            "section",
            { class: "card warnbox" },
            h("p", { class: "warn" }, () => reasonLine()),
            h("p", { class: "clifall" }, h("code", { class: "mono" }, () => cliLine())),
          ),
      ),
      createShow(
        () => canLearn(),
        () =>
          // v14: this was eleven lines of prose sitting between the slot rail
          // and the controller — the manual, printed on the wall, in front of
          // the thing it describes. The one sentence you need to start stays
          // visible; the rest is one click away and does not push the hero
          // down the page.
          h(
            "details",
            { class: "hint" },
            h(
              "summary",
              null,
              h(
                "span",
                { class: "hintlead" },
                "Click a control, then press the panel key for it.",
              ),
            ),
            h(
              "p",
              { class: "hintbody" },
              "Esc or a click outside cancels, Delete clears. A control that ",
              "already has a key offers “Add another key” too, so several keys ",
              "can drive one control (press any of them); each key in the ",
              "Bindings list below carries its own ✕ that removes only that ",
              "key. Ctrl-click (or “Select multiple”) picks several controls ",
              "and maps them all to ONE key. Saves are immediate — nothing ",
              "asks “are you sure?”; every action reports itself with an Undo ",
              "button (Ctrl-Z undoes the newest) — and a running session takes ",
              "them live without unplugging the pads.",
            ),
          ),
      ),
      // ── THE CONTROLLER (huge). Art + zone layer per persona. ──────────
      h(
        "section",
        { class: "card stagecard" },
        // FEATURE 2's discoverable half: a "Select multiple" toggle in the
        // card header (Victor: "tick something"). Hidden until map.ts marks
        // the island `.js` — with JS off it would be a dead button, and this
        // page's rule is that nothing ever looks clickable and does nothing.
        h(
          "div",
          { class: "stagehead" },
          h("h2", null, "Controller"),
          h(
            "button",
            {
              class: () => selToggleCls(),
              "data-act": "multi-toggle",
              type: "button",
              title: "Select several controls, then map them all to one key",
            },
            () => selToggleLabel(),
          ),
        ),
        createShow(
          () => artXbox(),
          () =>
            h(
              "div",
              { class: "stage stage-xbox" },
              h("img", {
                class: "padart",
                src: "/_assets/pad-xbox.svg",
                alt: "Xbox-style controller",
              }),
              h(
                "div",
                { class: "zonelayer" },
                createList(
                  () => zones(),
                  (z) => z.fn + "|" + z.cls + "|" + z.style + "|" + z.title + "|" + z.tag,
                  (z) =>
                    h(
                      "button",
                      {
                        class: z.cls,
                        style: z.style,
                        "data-fn": z.fn,
                        type: "button",
                        title: z.title,
                        "aria-label": z.title,
                      },
                      // FEATURE 1: identity first (the art draws no letters),
                      // binding key underneath it. Both are bare `param.field`
                      // reads — the supported per-item attr path, ledger #11.
                      h("span", { class: z.idcls }, z.id),
                      h("span", { class: "ztag" }, z.tag),
                    ),
                ),
              ),
            ),
        ),
        createShow(
          () => artDs4(),
          () =>
            h(
              "div",
              { class: "stage stage-ds4" },
              h("img", {
                class: "padart",
                src: "/_assets/pad-ds4.svg",
                alt: "DualShock 4 controller",
              }),
              h(
                "div",
                { class: "zonelayer" },
                createList(
                  () => zones(),
                  (z) => z.fn + "|" + z.cls + "|" + z.style + "|" + z.title + "|" + z.tag,
                  (z) =>
                    h(
                      "button",
                      {
                        class: z.cls,
                        style: z.style,
                        "data-fn": z.fn,
                        type: "button",
                        title: z.title,
                        "aria-label": z.title,
                      },
                      // FEATURE 1: identity first (the art draws no letters),
                      // binding key underneath it. Both are bare `param.field`
                      // reads — the supported per-item attr path, ledger #11.
                      h("span", { class: z.idcls }, z.id),
                      h("span", { class: "ztag" }, z.tag),
                    ),
                ),
              ),
            ),
        ),
      ),
      // ── Bindings legend: the readable truth below the stage. One row per
      // mappable function; a row click IS the zone click (same data-fn
      // delegation → learn modal), hover cross-highlights the zone. Renders
      // server-side too, so no-JS users still read their bindings here. ──
      h(
        "section",
        { class: "card legendcard" },
        h("h2", null, "Bindings"),
        h(
          "div",
          { class: "legend" },
          createList(
            () => legendRows(),
            (l) =>
              l.fn + "|" + l.label + "|" + l.key + "|" + l.cls + "|" + l.clear + "|" + l.share,
            (l) =>
              h(
                "div",
                { class: "lrowwrap" },
                h(
                  "button",
                  { class: l.cls, "data-fn": l.fn, type: "button", title: l.title },
                  // The same identity glyph the art wears, so the two readers
                  // are visibly the same control.
                  h("span", { class: l.idcls }, l.id),
                  h("span", { class: "llabel" }, l.group),
                  // MANY KEYS → ONE CONTROL: one chip per key, each with its
                  // own ✕ that removes JUST that key and leaves the others.
                  // Fixed chips, not a nested list — a `createList` inside a
                  // list item has no seam — and `lkc off` is how an unused
                  // chip disappears (`:empty` cannot: the SSR text slot leaves
                  // marker nodes inside the span; ledger #15).
                  h("span", { class: l.k1cls, title: l.k1title }, l.k1),
                  h("span", { class: l.k1xcls, "data-rmkey": l.k1rm, title: l.k1title }, "✕"),
                  h("span", { class: l.k2cls, title: l.k2title }, l.k2),
                  h("span", { class: l.k2xcls, "data-rmkey": l.k2rm, title: l.k2title }, "✕"),
                  h("span", { class: l.k3cls, title: l.k3title }, l.k3),
                  h("span", { class: l.k3xcls, "data-rmkey": l.k3rm, title: l.k3title }, "✕"),
                  h("span", { class: l.kmorecls, title: l.kmoretitle }, l.kmore),
                  // Unbound rows still say so in words; the chips above are
                  // empty and CSS keeps this one for exactly that case.
                  h("span", { class: "lkey" }, l.key),
                  // The desktop accelerator (revealed on hover/focus, always
                  // present for keyboard and AT users). Never the ONLY way to
                  // clear: the learn modal's button is the touch-first path.
                  h(
                    "span",
                    { class: "lclear", "data-clear": l.fn, title: l.cleartitle },
                    l.clear,
                  ),
                  // FEATURE 3: a shared key is information. This names the other
                  // controls the same key drives, on its own line so a
                  // multi-bound row grows instead of squeezing.
                  h("span", { class: "lshare", title: l.sharetitle }, l.share),
                  // AUTO-FIRE (§3). Its own badge on its own line, like the
                  // shared-key one: a row that auto-fires GROWS instead of
                  // squeezing, and a row that does not renders an empty span
                  // the CSS collapses (never a show — ledger #13/#14).
                  h("span", { class: "lturbo", title: l.turbotitle }, l.turbo),
                ),
                // ── v9: the row's own no-JS write path ──────────────────
                // A real HTML form: pick a key, submit, the server writes it
                // and 303s back with the outcome as ?flash=. `.nojs` is
                // hidden the moment map.ts marks the island `.js`, because
                // with JavaScript the click-to-learn flow above is better in
                // every way (it hears the actual panel button). Clear rides
                // the same form through `formaction` — one form, two verbs,
                // no duplicated hidden fields.
                h(
                  "form",
                  { class: l.bindcls, method: "post", action: "/map/bind" },
                  h("input", { type: "hidden", name: "slot", value: l.slot }),
                  h("input", { type: "hidden", name: "function", value: l.fn }),
                  h(
                    "select",
                    { class: "keysel", name: "key", title: l.bindtitle, "aria-label": l.bindtitle },
                    h("option", { value: "" }, "key…"),
                    h(
                      "optgroup",
                      { label: "Letters" },
                      ...KEYS_LETTER.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Digits (One = the 1 key)" },
                      ...KEYS_DIGIT.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Arrows" },
                      ...KEYS_ARROW.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Numpad" },
                      ...KEYS_NUMPAD.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Function keys" },
                      ...KEYS_FN.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Editing" },
                      ...KEYS_EDIT.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Navigation" },
                      ...KEYS_NAV.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Modifiers" },
                      ...KEYS_MOD.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Symbols (DashUnderscore = the - key)" },
                      ...KEYS_SYMBOL.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "Media and system" },
                      ...KEYS_MEDIA.map((ko) => h("option", null, ko.k)),
                    ),
                    h(
                      "optgroup",
                      { label: "OEM / regional" },
                      ...KEYS_OEM.map((ko) => h("option", null, ko.k)),
                    ),
                  ),
                  h("button", { class: "btn btn-mini", type: "submit" }, "Bind"),
                  // v10, no-JS parity for MANY KEYS → ONE CONTROL. Same form,
                  // same picker, two more destinations: Add appends the picked
                  // key to what the control already holds, Remove takes just
                  // that one off it. (Removing one of several without
                  // JavaScript needs to name WHICH key — the picker beside
                  // these buttons is that name, so no second control is
                  // needed and no per-key form has to be rendered 25 times.)
                  h(
                    "button",
                    {
                      class: "btn btn-mini",
                      type: "submit",
                      formaction: "/map/add",
                      title: l.addtitle,
                    },
                    "Add",
                  ),
                  h(
                    "button",
                    {
                      class: "btn btn-mini",
                      type: "submit",
                      formaction: "/map/key/remove",
                      title: l.rmtitle,
                    },
                    "Remove key",
                  ),
                  h(
                    "button",
                    {
                      class: "btn btn-mini",
                      type: "submit",
                      formaction: "/map/clear",
                      title: l.cleartitle,
                    },
                    "Clear",
                  ),
                  // v13, no-JS parity for AUTO-FIRE. Same form, one more
                  // destination and one more field: the number of presses a
                  // second this control should fire at while its key is held.
                  // `0` is off, in the same units as everything else, and the
                  // key picker beside it is not consulted — turbo belongs to
                  // the CONTROL, and the write keeps whatever keys it has.
                  h("input", {
                    class: "turboin",
                    type: "number",
                    name: "turbo_hz",
                    min: "0",
                    step: "1",
                    value: l.turboval,
                    placeholder: "Hz",
                    "aria-label": l.turbotitle,
                    title: l.turbotitle,
                  }),
                  h(
                    "button",
                    {
                      class: "btn btn-mini",
                      type: "submit",
                      formaction: "/map/turbo",
                      title: l.turbotitle,
                    },
                    "Turbo",
                  ),
                ),
              ),
          ),
        ),
      ),
      // ── v11: THE MACRO EDITOR — the piano roll ────────────────────────
      // rows = steps, columns = this pad's controls, a cell is held or not
      // (docs/INPUT-TRANSFORMS.md §6.2). NOT a createShow anywhere in here:
      // every state is a class string on an element that is always in the
      // DOM, so MAP_SHOW_ORDER does not move (ledger #4/#14).
      // v14: a <details>, closed on arrival. Everything it holds is still
      // here and still SSR-rendered (a closed disclosure is markup, not a
      // removal — the no-JS reader opens it with one click), but a piano roll,
      // four policy explainers and a TOML block no longer occupy 40 % of the
      // page in front of a user who came to map a button. Not a createShow:
      // MAP_SHOW_ORDER does not move (ledger #4/#14).
      h(
        "details",
        { class: () => macroCardCls() },
        h(
          "summary",
          null,
          h("span", { class: "sumtitle" }, "Macros"),
          h("span", { class: "sumnote mono" }, () => macroHead()),
        ),
        h(
          "div",
          { class: "phead" },
          h("h2", { class: "sr-head" }, "This preset's macros"),
          // THE save. One button, always in the same place, and its class says
          // whether there is anything to write — the answer to "why can't you
          // just save it? do I need to go to a folder and open it up?".
          h(
            "button",
            {
              class: () => macroSaveCls(),
              "data-act": "macro-save",
              type: "button",
              title: "write this whole macro into the preset file",
            },
            "Save macro",
          ),
          // The same fact in words, AFTER the button on purpose: CSS can then
          // colour it from the button's own dirty class (`.macsave.dirty +
          // .macdirty`), so "unsaved" is amber and "saved" is quiet without a
          // second signal.
          h("span", { class: "macdirty mono" }, () => macroDirtyLine()),
        ),
        // FIX 2: Save's question about steps below the sampling floor. Static
        // in the DOM, hidden by its own class until Save asks — no createShow
        // (ledger #4/#14), and its two buttons are the whole answer.
        h(
          "div",
          { class: () => macroConfirmCls() },
          h("p", { class: "macconfirmline" }, () => macroConfirmLine()),
          h(
            "span",
            { class: "macconfirmbtns" },
            h(
              "button",
              {
                class: "btn btn-mini macsaveyes",
                "data-act": "macro-save-anyway",
                type: "button",
                title: "write the macro exactly as it stands, short steps and all",
              },
              "Save anyway",
            ),
            h(
              "button",
              {
                class: "btn btn-mini macsaveno",
                "data-act": "macro-save-cancel",
                type: "button",
                title: "leave the preset alone and go back to the grid",
              },
              "Not yet",
            ),
          ),
        ),
        // FIX 1 — THE ONE SENTENCE, permanent, above everything it explains.
        // Victor's evening was lost to a "hadouken" whose rows were ↓ then →
        // then X: a diagonal is not an input you bind, it is down and forward
        // held TOGETHER, which is one row with two cells lit. The grid never
        // said so, and a fact this load-bearing does not go behind a
        // disclosure, a tooltip or a hover.
        h(
          "p",
          { class: "macconcept" },
          "A step holds everything you tick in that row AT ONCE — a diagonal ",
          "is ONE step holding ↓ and →, not two steps. So pick the diagonal: ",
          "every direction group has ↖ ↗ ↙ ↘ of its own, and ticking ↘ stores ",
          "dpad.down + dpad.right, which is what a diagonal has always been in ",
          "the file. Every row says what it holds, in words, beside its number, ",
          "and spells out the pair it wrote.",
        ),
        // What this card IS, before any of its controls. A first-time reader
        // should not have to open docs/INPUT-TRANSFORMS.md to use it.
        h(
          "p",
          { class: "savenote" },
          "A MACRO is a timed sequence the pad plays by itself: each row below ",
          "is one step, the columns are this pad's controls, and a step holds ",
          "whatever its row has filled in — for its own duration — before the ",
          "next one starts. A quarter-circle is three steps: ↓, then ↘ (which ",
          "is ↓ and → together on ONE row), then →. A TRIGGER is the ",
          "panel key that STARTS the macro: bind one in the Trigger section at ",
          "the bottom, and from then on that single key press plays the whole ",
          "sequence. The two are separate on purpose — the sequence lives in ",
          "the preset's [macros] table, the trigger is an ordinary binding ",
          "pointing at it.",
        ),
        h("p", { class: "savenote" }, () => macroNote()),
        h(
          "div",
          { class: "mactabs" },
          createList(
            () => macroTabs(),
            (t) => t.name + "|" + t.cls + "|" + t.label,
            // An ANCHOR, like the slot tabs: `/map?slot=N&macro=NAME` is a
            // route, so a page with no JavaScript can still walk every macro
            // the preset defines. map.ts intercepts and switches in place.
            (t) => h("a", { class: t.cls, href: t.href, "data-macro": t.name }, t.label),
          ),
        ),
        // CREATE. A name, validated, and a button that WRITES the macro into
        // the preset (one empty 50 ms step) — so a macro on this card is never
        // a thing that exists only in the browser, and its trigger is always
        // bindable. JS-only, like every other JSON verb here; without
        // JavaScript the TOML block at the bottom is still the way in.
        h(
          "div",
          { class: "macnewbox" },
          h(
            "label",
            { class: "bindlabel" },
            "new macro name",
            h("input", {
              class: "macnewin",
              type: "text",
              placeholder: "e.g. hadouken",
              "aria-label": "new macro name",
            }),
          ),
          h(
            "button",
            {
              class: "btn btn-mini macnew",
              "data-act": "macro-new",
              type: "button",
              title: "create this macro in the preset now",
            },
            "＋ New macro",
          ),
        ),
        h("p", { class: "machead" }, () => macroHead()),
        // The slot-wide switch, above everything it silences. Empty (and so
        // invisible) on every slot that runs macros — see `slotMacrosLine` for
        // why this is a sentence and not a button.
        h("p", { class: "macslotoff" }, () => slotMacrosLine()),
        h("p", { class: "macpolicy mono" }, () => macroPolicyLine()),
        // FIX 1c — COMMON MOTIONS. These are the sequences everybody is
        // actually trying to build, and they are exactly where the
        // two-controls-per-row concept bites: the middle step of a
        // quarter-circle IS the diagonal. Each button appends a correct step
        // group at 50 ms (above the sampling floor), written on the mechanism
        // this slot's own direction keys drive — see `macroMotionLineFor`, and
        // `Issue::MacroHoldsOtherMechanism` for the trap it sidesteps.
        //
        // JS-only, like every other draft edit here.
        //
        // FIX 3 — THE LABELS SPEAK DIAGONALS. They used to spell the holds
        // ("¼ → · ↓ · ↓+→ · →") because an earlier pass wanted the buttons to
        // teach that one row can hold several controls. First-class diagonals
        // landed since, and `↓+→` now CONTRADICTS the abstraction the grid,
        // the row readout and the ↘ column all use: a diagonal is one control,
        // not two held together. So a motion reads as the shape a player
        // already knows — `↓ ↘ →` — and the model is taught in exactly one
        // place, the row's own expansion ledger (`↘ = dpad.down + dpad.right`),
        // which every generated step carries the moment the button is pressed.
        h(
          "div",
          { class: "macmotions" },
          h("span", { class: "macmotlbl" }, "common motions"),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "qcf",
              type: "button",
              // Wrapped across lines again on 2026-08-06: a concatenation in
              // ATTRIBUTE position used to emit the attribute with NO VALUE AT
              // ALL — the tooltip silently disappeared, which is how both 360
              // buttons shipped without one until FIX 3's pwtest case read the
              // titles back and found `null`. Fixed in @getforma/compiler
              // 0.3.1 (docs/FORMA-DOGFOOD.md #20a); the pwtest case is what
              // keeps it fixed.
              title:
                "append a quarter-circle forward: ↓ ↘ → — three steps, the " +
                "middle one the diagonal (each row spells the pair it stores)",
            },
            "¼ → · ↓ ↘ →",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "qcb",
              type: "button",
              title: "append a quarter-circle back: ↓ ↙ ← — three steps, the middle one the diagonal (each row spells the pair it stores)",
            },
            "¼ ← · ↓ ↙ ←",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "hcf",
              type: "button",
              title: "append a half-circle forward: ← ↙ ↓ ↘ → — five steps, two of them diagonals",
            },
            "½ → · ← ↙ ↓ ↘ →",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "hcb",
              type: "button",
              title: "append a half-circle back: → ↘ ↓ ↙ ← — five steps, two of them diagonals",
            },
            "½ ← · → ↘ ↓ ↙ ←",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "dpf",
              type: "button",
              title: "append a dragon punch forward: → ↓ ↘ — three steps, the last one the diagonal",
            },
            "DP → · → ↓ ↘",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "dpb",
              type: "button",
              title: "append a dragon punch back: ← ↓ ↙ — three steps, the last one the diagonal",
            },
            "DP ← · ← ↓ ↙",
          ),
          // THE FULL CIRCLE — the motion that needs all four diagonals, and the
          // reason they are first-class rather than just ↘. Eight steps, one
          // per position of the gate, four of them diagonals.
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "spdf",
              type: "button",
              title:
                "append a full 360 (spinning piledriver), clockwise from →: " +
                "→ ↘ ↓ ↙ ← ↖ ↑ ↗ — eight steps, four of them diagonals",
            },
            "360 → · → ↘ ↓ ↙ ← ↖ ↑ ↗",
          ),
          h(
            "button",
            {
              class: "btn btn-mini macmot",
              "data-macmotion": "spdb",
              type: "button",
              title:
                "append a full 360 the other way round, from ←: " +
                "← ↙ ↓ ↘ → ↗ ↑ ↖ — eight steps, four of them diagonals",
            },
            "360 ← · ← ↙ ↓ ↘ → ↗ ↑ ↖",
          ),
          h("p", { class: "macmotnote" }, () => macroMotionLine()),
        ),
        // FIX 1 — THE STEP TOOLBAR, and the rule it exists for: an add
        // affordance must not live on a row. Every other way to grow a macro
        // (insert-above, insert-below) hangs off an existing step, which is
        // fine as long as one exists — and Victor pressed ✕ until none did.
        // Deleting the last step no longer empties the grid (`macroStepVerb`
        // says why), so this bar is no longer the ONLY road back; it is still
        // the one that does not depend on the grid having anything in it, and
        // it sits ABOVE the grid where the thing it adds to is visible.
        //
        // NOT a multi-select bar, deliberately. Bulk editing of rows was
        // floated and refused: multi-select is a MODE, and the mode
        // (select-then-edit) is precisely what FIX 2 just removed from the
        // duration editor. If bulk edits are ever really needed, drag-select
        // across rows is the gesture — no toolbar toggle.
        h(
          "div",
          { class: "macsteptools" },
          h(
            "button",
            {
              class: "btn btn-mini macaddstep",
              "data-act": "macro-addstep",
              type: "button",
              title: "append a new 50 ms step, holding nothing yet, at the end of the macro",
            },
            "＋ Add step",
          ),
          // One concatenated child again (2026-08-06). Until compiler 0.3.1
          // a `"a" + "b"` in CHILD position was a BinaryExpression the h-tree
          // walker could not fold, so it emitted an anonymous SECOND ISLAND —
          // caught, loudly, by `embedded_map_ir_slot_layout_matches_the_seam`'s
          // "expected exactly one island". docs/FORMA-DOGFOOD.md #20b.
          h(
            "span",
            { class: "macstephint" },
            "…or ＋↑ / ＋↓ on a row to insert next to it. ✕ deletes a step — on the " +
              "last one it empties it instead, because a macro with no steps is not " +
              "something ksx can save.",
          ),
        ),
        // The grid. Two aligned columns: the row bar (step number, duration,
        // amber flag, the five step verbs) and the scrollable matrix with its
        // control headers. Row heights are fixed in CSS so the two line up.
        h(
          "div",
          { class: () => macroGridCls() },
          h(
            "div",
            { class: "macrowbar" },
            h("div", { class: "macrowhead" }, "step"),
            createList(
              () => macroRows(),
              // The KEY, and what is deliberately not in it: the row rebuilds
              // when its words change (`dur` carries the unit conversion the
              // `unit` toggle performs, and `deltitle` flips on the last
              // remaining step), but the duration BOX's value never comes from
              // here — map.ts writes it, skipping whichever box has the caret,
              // exactly like every other form control on this card.
              (r) =>
                r.n +
                "|" +
                r.cls +
                "|" +
                r.dur +
                "|" +
                r.unit +
                "|" +
                r.deltitle +
                "|" +
                r.warn +
                "|" +
                r.hold +
                "|" +
                r.holdcls +
                "|" +
                r.exp,
              (r) =>
                h(
                  "div",
                  { class: r.cls, title: r.durtitle },
                  h("span", { class: "macnum" }, r.n),
                  // WHAT THIS ROW HOLDS, before its timing — reading the grid
                  // must never mean decoding which of 37 columns are lit. A
                  // diagonal reads as ONE control here, because that is what
                  // was picked and what it means; `machold both` is the accent
                  // for a row that really does hold several things at once.
                  h("span", { class: r.holdcls }, r.hold),
                  // …and THE LEDGER beside it: the two names the file carries
                  // for that diagonal. The lens is honest only if the storage
                  // is visible without opening the TOML.
                  h("span", { class: r.expcls, title: r.exp }, r.exp),
                  // THE DURATION, on the row it belongs to. The words are the
                  // no-JS readout; with JavaScript the box beside them takes
                  // over (CSS swaps the two on `.studio.js`) so a time is
                  // edited exactly where it is read — no step to select first,
                  // no single field under the grid to go and find.
                  h("span", { class: "macdur" }, r.dur),
                  h(
                    "span",
                    { class: "macdured" },
                    h("input", {
                      class: r.durcls,
                      "data-durrow": r.durrow,
                      type: "number",
                      min: "1",
                      step: "1",
                      value: r.durval,
                      title: r.durtitle,
                    }),
                    h(
                      "button",
                      {
                        class: "macrowunit",
                        "data-macact": r.unitact,
                        type: "button",
                        title: r.unittitle,
                      },
                      r.unit,
                    ),
                  ),
                  // FLAGGED INLINE, in amber, with the reason — never a
                  // silent accept and never a silent rewrite (§0.2). The
                  // short form always fits; the whole sentence is the title,
                  // and the rule behind it is stated once below the grid.
                  h("span", { class: r.warncls, title: r.warntitle }, r.warn),
                  h(
                    "span",
                    { class: "macbtns" },
                    h(
                      "button",
                      { class: r.upcls, "data-macact": r.upact, type: "button", title: "move this step up" },
                      "▲",
                    ),
                    h(
                      "button",
                      { class: r.dncls, "data-macact": r.dnact, type: "button", title: "move this step down" },
                      "▼",
                    ),
                    h(
                      "button",
                      { class: "macbtn", "data-macact": r.iaact, type: "button", title: "insert a step above this one" },
                      "＋↑",
                    ),
                    h(
                      "button",
                      { class: "macbtn", "data-macact": r.ibact, type: "button", title: "insert a step below this one" },
                      "＋↓",
                    ),
                    // NOT "edit this step's duration" any more — that is what
                    // the box two elements left of here is for. This focuses
                    // the row so the detail line under the grid (its frame
                    // maths, its allow-short box) is about it.
                    h(
                      "button",
                      {
                        class: "macbtn",
                        "data-macact": r.selact,
                        type: "button",
                        title: "show this step's frame maths below the grid",
                      },
                      "⏱",
                    ),
                    h(
                      "button",
                      { class: "macbtn macdel", "data-macact": r.delact, type: "button", title: r.deltitle },
                      "✕",
                    ),
                  ),
                ),
            ),
          ),
          h(
            "div",
            { class: "macscroll" },
            // v16: the GROUP BAND. Not decoration — it fixes a pre-existing
            // defect: the header used to carry three identical direction runs
            // told apart only by a tooltip, and with eight columns per group
            // instead of four that would be three identical EIGHTS.
            h(
              "div",
              { class: "macgrps" },
              createList(
                () => macroGroups(),
                (g) => g.label + "|" + g.cls,
                (g) => h("span", { class: g.cls }, g.label),
              ),
            ),
            h(
              "div",
              { class: "maccols" },
              createList(
                () => macroCols(),
                (c) => c.fn + "|" + c.id,
                (c) => h("span", { class: c.idcls, title: c.title }, c.id),
              ),
            ),
            h(
              "div",
              { class: "macmatrix" },
              createList(
                () => macroCells(),
                (c) => c.cell + "|" + c.cls,
                (c) =>
                  h(
                    "button",
                    { class: c.cls, "data-cell": c.cell, type: "button", title: c.title },
                    c.mark,
                  ),
              ),
            ),
          ),
        ),
        // The ring, and what a diagonal pick WRITES — stated once, under the
        // grid it describes, and SSR'd like everything else: a page with no
        // JavaScript cannot tick a cell, but it can read what the columns mean
        // and hand-write the pair into the TOML block below.
        h("p", { class: "macring" }, () => macroRingLine()),
        h("p", { class: "macrule" }, () => macroRuleLine()),
        // The step editor: everything here writes the DRAFT, so it only
        // exists with JavaScript (`.macedit` is display:none until map.ts
        // marks the island `.js`) — a control that cannot do anything is the
        // one thing this page never renders.
        h(
          "div",
          { class: "macedit" },
          h("span", { class: "macsteplbl" }, () => macroStepLine()),
          // FIX 2: the duration field and the unit select USED TO LIVE HERE,
          // pointed at whichever step was selected. That was the mode — a time
          // could not be changed without first picking a row, and every poll
          // that dropped the selection dropped the edit with it. Both controls
          // are now on every row (`.macdured`), where the number they change
          // is the number being read. What stays here is what is genuinely
          // ABOUT one step at a time: its frame maths, and its allow-short
          // flag, which follow the row you last touched rather than gating it.
          // The target rate the AUTHOR is thinking in. Display-only — nothing
          // stores a rate, and ksx counts `frames` at 60 Hz — which the math
          // line below says out loud whenever this is not 60.
          h(
            "label",
            { class: "bindlabel" },
            "game runs at",
            h(
              "select",
              { class: "macrate", title: "used only to convert frames ↔ ms while you author" },
              h("option", null, "60"),
              h("option", null, "59.94"),
              h("option", null, "57"),
              h("option", null, "55"),
              h("option", null, "50"),
              h("option", null, "30"),
            ),
          ),
          h(
            "label",
            { class: "macshortlbl" },
            h("input", { class: "macshortin", type: "checkbox" }),
            "allow short (run it as written even below 33 ms)",
          ),
          // THE MATH, live (Victor: "maybe we can show that math"). It carries
          // the sampling floor in the same units the focused row is authored
          // in, so an amber row explains itself.
          h("p", { class: "macmath mono" }, () => macroMathLine()),
          // "Add step at end" used to be here, at the bottom of a panel below
          // the grid. It is now the step toolbar ABOVE the grid — one add
          // button, in the place where somebody who has just emptied a row is
          // already looking.
          h(
            "button",
            { class: "btn btn-mini", "data-act": "macro-revert", type: "button" },
            "Revert to file",
          ),
          // RENAME is a real write: save under the new name, then delete the
          // old table, then move the trigger keys across — one action, one
          // toast, one Undo (map.ts). Typing here changes nothing on its own.
          h(
            "label",
            { class: "bindlabel" },
            "name",
            h("input", {
              class: "macnamein",
              type: "text",
              value: () => macroName(),
              "aria-label": "macro name",
            }),
          ),
          h(
            "button",
            {
              class: "btn btn-mini macrename",
              "data-act": "macro-rename",
              type: "button",
              title: "save this macro under the name in the box and remove the old table",
            },
            "Rename",
          ),
          // The SWITCH, next to Delete on purpose: they are the two answers to
          // "I do not want this macro right now", and the cheap one should be
          // the one you reach for. Disabling keeps the steps and the trigger
          // row; deleting takes both.
          h(
            "button",
            {
              class: () => macroEnableCls(),
              "data-act": "macro-enable",
              type: "button",
              // Concatenated again (ledger #20a, fixed in compiler 0.3.1 —
              // this used to emit the attribute with no value at all).
              title:
                "switch this macro off (or back on) without losing it — the " +
                "steps and the key that starts it stay exactly where they are. " +
                "Disable one to TEST the others, or the lot for a tournament",
            },
            () => macroEnableLabel(),
          ),
          h(
            "button",
            {
              class: "btn btn-mini macdelmac",
              "data-act": "macro-delete",
              type: "button",
              title: "delete this macro from the preset (its trigger rows go with it)",
            },
            "Delete macro",
          ),
        ),
        // The three interruption policies. The SELECTS are draft controls, so
        // they live in `.macedit` too; the one-line explanations and the
        // current values (the `macpolicy` line above) are there for everyone.
        h(
          "div",
          { class: "macpolicies" },
          h(
            "div",
            { class: "macpol" },
            h(
              "label",
              { class: "bindlabel macjs" },
              "on release",
              h(
                "select",
                { class: "macsel", "data-macpol": "on_release" },
                h("option", null, "finish"),
                h("option", null, "abort"),
              ),
            ),
            h(
              "span",
              { class: "machint" },
              "letting go of the trigger mid-run: finish runs the sequence out (the ",
              "fighting-game expectation — you tap the button and the quarter-circle ",
              "comes out whole), abort stops it and releases everything in one batch.",
            ),
          ),
          h(
            "div",
            { class: "macpol" },
            h(
              "label",
              { class: "bindlabel macjs" },
              "retrigger",
              h(
                "select",
                { class: "macsel", "data-macpol": "retrigger" },
                h("option", null, "ignore"),
                h("option", null, "restart"),
              ),
            ),
            h(
              "span",
              { class: "machint" },
              "pressing the trigger again mid-run: ignore swallows the press (the ",
              "default, because restart stutters the sequence back to step 0 on any ",
              "switch bounce a real panel has), restart starts over from step 1.",
            ),
          ),
          h(
            "div",
            { class: "macpol" },
            h(
              "label",
              { class: "bindlabel macjs" },
              "interrupt",
              h(
                "select",
                { class: "macsel", "data-macpol": "interrupt" },
                h("option", null, "none"),
                h("option", null, "any-input"),
                h("option", null, "opposing"),
              ),
            ),
            h(
              "span",
              { class: "machint" },
              "doing something ELSE mid-run: none never interrupts, any-input aborts ",
              "on any other bound key of this slot going down, opposing aborts only on ",
              "input that contradicts the macro — a direction against one the current ",
              "step holds, or a key that starts a different macro.",
            ),
          ),
          // ── v13: AUTOREPEAT — the option Victor went looking for ──────
          // Same shape as the three above (a `.macsel` the generic
          // `data-macpol` delegation already routes), plus a rate field,
          // because "turbo" without a number is a table the loader refuses.
          h(
            "div",
            { class: "macpol" },
            h(
              "label",
              { class: "bindlabel macjs" },
              "repeat",
              h(
                "select",
                { class: "macsel", "data-macpol": "repeat" },
                h("option", null, "once"),
                h("option", null, "while-held"),
                h("option", null, "turbo"),
              ),
            ),
            h(
              "span",
              { class: "machint" },
              "what the END of a run does while the trigger is STILL held: once ",
              "stops (the default, and what keeps a special move from becoming a ",
              "machine gun), while-held runs it again immediately with no gap — for ",
              "a motion — and turbo runs it again with a deliberate NEUTRAL GAP, so ",
              "the game sees two presses instead of one long hold. That gap is the ",
              "whole difference, and it is why auto-fire needs a rate.",
            ),
            h(
              "label",
              { class: "bindlabel macjs" },
              "rate",
              h("input", {
                class: "macturboin",
                type: "number",
                min: "0",
                step: "1",
                value: () => macroTurboValue(),
                "aria-label": "turbo rate",
              }),
            ),
            h(
              "label",
              { class: "bindlabel macjs" },
              "unit",
              h(
                "select",
                {
                  class: "macturbounit",
                  title: "two spellings of one number — switching moves the value, never doubles it",
                },
                h("option", { value: "turbo_hz" }, "presses/sec (turbo_hz)"),
                h("option", { value: "gap_ms" }, "gap ms (gap_ms)"),
              ),
            ),
            // THE MATH, live — the same promise the duration field makes.
            h("p", { class: "macmath mono" }, () => macroTurboLine()),
          ),
        ),
        // ── The trigger: the ONE macro edit that is a real write ─────────
        // `macro.<name>` is a function name the `map` verb already takes
        // (mapping.rs `apply_macro_trigger`), so this goes through the same
        // writer as every other binding on the page — learn flow with
        // JavaScript, a plain form without it. No second writer, no fake one.
        h(
          "div",
          { class: () => macroTrigCls() },
          h("h3", null, "Trigger — the key that STARTS this macro"),
          h(
            "p",
            { class: "savenote" },
            "This is an ordinary binding, saved the moment you set it: it points ",
            "the panel key at this macro instead of at a pad button, so pressing ",
            "it plays the sequence above from step 1. Several keys can start the ",
            "same macro. A macro with no trigger is inert — it exists in the ",
            "preset and nothing ever runs it.",
          ),
          h("p", { class: "mactrigline" }, () => macroTriggerLine()),
          h(
            "button",
            {
              class: "btn btn-row mactriglearn",
              "data-fn": () => macroFnName(),
              type: "button",
              title: "click, then press the panel key that should start this macro",
            },
            "Set trigger — press a panel key",
          ),
          // The no-JS twin. Bind REPLACES this macro's trigger keys and Clear
          // removes them; there is deliberately no Add/Remove-one here,
          // because the mapper payload's `bindings` map carries pad functions
          // only — the server's read-modify-write would compute the new set
          // against an empty list and quietly drop the triggers it never saw.
          // With JavaScript the page reads them from `[macros]` and can add.
          h(
            "form",
            { class: "macbind nojs", method: "post", action: "/map/bind" },
            h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
            h("input", { type: "hidden", name: "function", value: () => macroFnName() }),
            h(
              "select",
              {
                class: "keysel",
                name: "key",
                title: "the panel key that starts this macro",
                "aria-label": "the panel key that starts this macro",
              },
              h("option", { value: "" }, "key…"),
              h("optgroup", { label: "Letters" }, ...KEYS_LETTER.map((ko) => h("option", null, ko.k))),
              h(
                "optgroup",
                { label: "Digits (One = the 1 key)" },
                ...KEYS_DIGIT.map((ko) => h("option", null, ko.k)),
              ),
              h("optgroup", { label: "Arrows" }, ...KEYS_ARROW.map((ko) => h("option", null, ko.k))),
              h("optgroup", { label: "Numpad" }, ...KEYS_NUMPAD.map((ko) => h("option", null, ko.k))),
              h("optgroup", { label: "Function keys" }, ...KEYS_FN.map((ko) => h("option", null, ko.k))),
              h("optgroup", { label: "Editing" }, ...KEYS_EDIT.map((ko) => h("option", null, ko.k))),
              h("optgroup", { label: "Navigation" }, ...KEYS_NAV.map((ko) => h("option", null, ko.k))),
              h("optgroup", { label: "Modifiers" }, ...KEYS_MOD.map((ko) => h("option", null, ko.k))),
              h(
                "optgroup",
                { label: "Symbols (DashUnderscore = the - key)" },
                ...KEYS_SYMBOL.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Media and system" },
                ...KEYS_MEDIA.map((ko) => h("option", null, ko.k)),
              ),
              h("optgroup", { label: "OEM / regional" }, ...KEYS_OEM.map((ko) => h("option", null, ko.k))),
            ),
            h("button", { class: "btn btn-mini", type: "submit" }, "Bind trigger"),
            h(
              "button",
              { class: "btn btn-mini", type: "submit", formaction: "/map/clear" },
              "Clear trigger",
            ),
          ),
          h("p", { class: "clifall" }, h("code", { class: "mono copyable" }, () => macroCliLine())),
        ),
        // ── Advanced: the TOML block ─────────────────────────────────────
        // DEMOTED in v12 and collapsed by default. It was the only way to keep
        // a macro before the save path was wired, which is why the card used
        // to end with "copy this and go find the file". Save macro does that
        // now; this stays for sharing a sequence with someone else and for
        // hand-editing the preset — secondary, and it looks it.
        h(
          "details",
          { class: "mactomlbox" },
          h("summary", null, "Advanced — this macro as TOML (for sharing, or hand-editing the file)"),
          h(
            "p",
            { class: "savenote" },
            "You do not need this to keep your work: Save macro writes the same ",
            "table into the preset for you. Copy it to send a sequence to ",
            "someone else, or to paste it into presets\\<preset>.toml by hand ",
            "(the Preset card above names the config root).",
          ),
          h("pre", { class: "mono mactoml" }, () => macroToml()),
          h(
            "button",
            { class: "btn btn-mini maccopy", "data-act": "macro-copy", type: "button" },
            "Copy",
          ),
        ),
      ),
      // ── v9: bind by name (no-JavaScript panel) ────────────────────────
      // The row forms above are the precise path; this is the one that does
      // not make you hunt through 25 of them. Same two verbs, same 303 →
      // ?flash= report. Hidden the moment the island is marked `.js`.
      // Not a createShow — a plain section whose visibility is CSS, so it
      // costs zero MAP_SHOW_ORDER entries (ledger #4/#14).
      h(
        "section",
        { class: "card nojs bindcard" },
        h("h2", null, "Bind by name"),
        h(
          "p",
          { class: "savenote" },
          "This panel exists so the mapper works with JavaScript switched off: ",
          "clicking a control and pressing its panel key needs a live poller, ",
          "picking a key from a list does not. Every row in the Bindings list ",
          "above carries the same four buttons. Writes go over the same daemon ",
          "verb as everything else and are saved immediately. Bind REPLACES ",
          "whatever the control had; Add keeps it and adds one more key, so ",
          "several keys can drive one control (press any of them); Remove that ",
          "key takes only the key picked above and leaves the rest.",
        ),
        h(
          "p",
          { class: "savenote" },
          "Both lists spell things exactly as the preset file and ",
          "`ksx map` do: lx / ly are the left stick and rx / ry the right, ",
          ".min is left or down and .max is right or up, and a key is its ",
          "legacy name (DashUnderscore is the - key, CommaLeftArrow the comma). ",
          "What you pick here is character-for-character what gets written.",
        ),
        h(
          "form",
          { class: "bindform", method: "post", action: "/map/bind" },
          h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
          h(
            "label",
            { class: "bindlabel", for: "bindfn" },
            "control",
            h(
              "select",
              { id: "bindfn", name: "function" },
              ...FUNCTIONS.map((fo) => h("option", null, fo.k)),
            ),
          ),
          h(
            "label",
            { class: "bindlabel", for: "bindkey" },
            "key",
            h(
              "select",
              { id: "bindkey", name: "key" },
              h("option", { value: "" }, "key…"),
              h(
                "optgroup",
                { label: "Letters" },
                ...KEYS_LETTER.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Digits (One = the 1 key)" },
                ...KEYS_DIGIT.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Arrows" },
                ...KEYS_ARROW.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Numpad" },
                ...KEYS_NUMPAD.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Function keys" },
                ...KEYS_FN.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Editing" },
                ...KEYS_EDIT.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Navigation" },
                ...KEYS_NAV.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Modifiers" },
                ...KEYS_MOD.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Symbols (DashUnderscore = the - key)" },
                ...KEYS_SYMBOL.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "Media and system" },
                ...KEYS_MEDIA.map((ko) => h("option", null, ko.k)),
              ),
              h(
                "optgroup",
                { label: "OEM / regional" },
                ...KEYS_OEM.map((ko) => h("option", null, ko.k)),
              ),
            ),
          ),
          h("button", { class: "btn btn-primary", type: "submit" }, "Bind"),
          h(
            "button",
            {
              class: "btn",
              type: "submit",
              formaction: "/map/add",
              title: "keep the keys this control already has and add the one picked above",
            },
            "Add another key",
          ),
          h(
            "button",
            {
              class: "btn",
              type: "submit",
              formaction: "/map/key/remove",
              title: "remove only the key picked above, leaving the control's other keys",
            },
            "Remove that key",
          ),
          h(
            "button",
            { class: "btn", type: "submit", formaction: "/map/clear" },
            "Clear this control",
          ),
          // The no-JS answer to "that key is already another slot's". The
          // learn flow asks with a dialog; a form asks with a checkbox, and
          // the refusal sentence tells you it is here.
          h(
            "label",
            { class: "bindforce" },
            h("input", { type: "checkbox", name: "force", value: "1" }),
            "let this key drive another slot's control too",
          ),
        ),
      ),
      // ── Preset actions: save semantics + the two restore safety nets.
      // Always rendered (a class string flips the inert look — never a
      // show, so its bindings survive; ledger #13). Buttons share map.ts's
      // data-act delegation; each confirms before the pipe verb. ──────────
      // ── PRESETS & FILES ──────────────────────────────────────────────
      // v14: this was a bare row of four buttons under a paragraph, and the
      // answer to "which file am I editing, which slots share it, and where
      // do backups go?" existed nowhere on the screen — Victor: "the save of
      // the files and profiles feels amateur". It is now a real management
      // surface: the preset's identity, every slot and the preset it binds,
      // then the actions, graded by consequence with the destructive one
      // pushed to the end of the row. No new verbs — the same four forms.
      h(
        "section",
        { class: () => actionsCls() },
        h(
          "div",
          { class: "phead" },
          h("h2", null, "Presets & files"),
          // Auto-save, made visible. Empty until this page writes something.
          h("span", { class: "savedat mono" }, () => savedAt()),
        ),
        h(
          "p",
          { class: "savenote" },
          "Every binding saves immediately — there is no Save button, and no ",
          "action asks “are you sure?”. Each one reports what it did and offers ",
          "Undo for a few seconds (Ctrl-Z takes the newest). The restore options ",
          "below are the wider road home, and every one of them writes a ",
          "timestamped backup first.",
        ),
        // What you are editing, and where it lives on disk.
        h(
          "div",
          { class: "presetid" },
          h("span", { class: "presetname mono" }, () => presetLine()),
          h(
            "span",
            { class: "presetfact" },
            h("b", null, "file"),
            h("span", null, () => presetPath()),
          ),
          h(
            "span",
            { class: "presetfact" },
            h("b", null, "backups"),
            h("span", null, () => backupFact()),
          ),
        ),
        // Every slot, the preset it binds and the keyboard that drives it —
        // the "which slots use this file?" read. Rows are the same anchors
        // the rail uses, so this table is also a way to switch slot.
        h(
          "div",
          { class: "slottable" },
          h(
            "div",
            { class: "strow sthead" },
            h("span", { class: "stcell stnum" }, "slot"),
            h("span", { class: "stcell stpreset" }, "preset"),
            h("span", { class: "stcell stpersona" }, "pad"),
            h("span", { class: "stcell stkbd" }, "keyboard"),
          ),
          createList(
            () => slotTabs(),
            (t) => t.num + "|" + t.rowcls + "|" + t.preset + "|" + t.pad + "|" + t.kbd,
            (t) =>
              h(
                "a",
                { class: t.rowcls, href: t.href, "data-slot": t.num, title: t.label },
                h("span", { class: "stcell stnum" }, t.player),
                h("span", { class: "stcell stpreset" }, t.preset),
                h("span", { class: "stcell stpersona" }, t.pad),
                h("span", { class: "stcell stkbd" }, t.kbd),
              ),
          ),
        ),
        // v9: every one of these is a REAL form now — method=post, a hidden
        // slot number, a submit button. With JavaScript off they POST and
        // 303 back with the outcome flashed; with it on, map.ts's data-act
        // delegation runs the richer toast+Undo path and the submit handler
        // stops the navigation. `.pactform { display: contents }` keeps the
        // row's flex layout exactly as it was.
        h(
          "div",
          { class: "pactrow" },
          h(
            "form",
            { class: "pactform", method: "post", action: "/map/preset/clear-all" },
            h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
            h(
              "button",
              { class: "btn btn-row", "data-act": "clear-all", type: "submit" },
              "Clear all bindings",
            ),
          ),
          h(
            "form",
            { class: "pactform", method: "post", action: "/map/preset/restore" },
            h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
            h("input", { type: "hidden", name: "mode", value: "session-backup" }),
            h(
              "button",
              { class: "btn btn-row", "data-act": "restore-backup", type: "submit" },
              "Undo this session",
            ),
          ),
          // FIX 2's third destination — only rendered when a backup exists,
          // because an offer of a road home that is not there is worse than
          // no offer. The timestamp is IN the label, not in a tooltip.
          createShow(
            () => hasBackup(),
            () =>
              h(
                "form",
                { class: "pactform", method: "post", action: "/map/preset/restore" },
                h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
                h("input", { type: "hidden", name: "mode", value: "latest-backup" }),
                h(
                  "button",
                  { class: "btn btn-row", "data-act": "restore-latest", type: "submit" },
                  () => backupLine(),
                ),
              ),
          ),
          // FIX 2: the label names the LAYOUT it writes. "Restore built-in
          // defaults" read, to Victor, as "put my I-PAC map back" — and wrote
          // a desktop-keyboard layout over it.
          h(
            "form",
            { class: "pactform", method: "post", action: "/map/preset/restore" },
            h("input", { type: "hidden", name: "slot", value: () => slotNum() }),
            h("input", { type: "hidden", name: "mode", value: "defaults" }),
            h(
              "button",
              {
                class: "btn btn-row btn-danger-ghost",
                "data-act": "restore-defaults",
                type: "submit",
              },
              "Reset to generic keyboard layout (S/D/A/W…)",
            ),
          ),
        ),
      ),
      // ── Save feedback ─────────────────────────────────────────────────
      createShow(
        () => savedOk(),
        () => h("p", { class: "flash flash-ok" }, () => savedLine()),
      ),
      createShow(
        () => savedErr(),
        () => h("p", { class: "flash flash-err" }, () => savedLine()),
      ),
    ),
    // ── The learn modal (client-only; never SSR-open) ─────────────────────
    createShow(
      () => modalOpen(),
      () =>
        h(
          "div",
          { class: "mlayer", "data-cancel": "1" },
          h(
            "div",
            { class: "modal" },
            h("h3", null, () => modalPrompt()),
            createShow(
              () => modalListening(),
              () =>
                h(
                  "div",
                  { class: "mbody" },
                  h("p", { class: "count mono" }, () => countdownText()),
                  h(
                    "div",
                    { class: "cdtrack" },
                    h("div", { class: "cdbar", style: () => barStyle() }),
                  ),
                  h(
                    "p",
                    { class: "mhint" },
                    "waiting for a key press on the panel… Esc or click outside to cancel",
                  ),
                ),
            ),
            // MAME's "UI Clear during capture", ported: the prompt that asks
            // for a new key is also where you say "none". Touch-first — the
            // legend's ✕ and the Delete key are accelerators, not the path.
            createShow(
              () => modalBound(),
              () =>
                h(
                  "div",
                  { class: "mbody mbound" },
                  h("p", { class: "mcurrent mono" }, () => modalBinding()),
                  // v10: an already-bound control offers BOTH outcomes for the
                  // key that is about to be pressed. REPLACE stays the primary
                  // and stays the default arm — it is what every mapper in the
                  // field study does on a rebind, it is what the user who
                  // clicked a bound control almost always means, and (unlike
                  // Add) it is expressible on every daemon. Add is the clearly
                  // labelled second option: press it, then press the panel key,
                  // and the control keeps what it had AND gains the new key.
                  // The armed choice is echoed in the line above so the modal
                  // never has a hidden mode.
                  h(
                    "div",
                    { class: "mbtns" },
                    h(
                      "button",
                      { class: "btn btn-primary", "data-act": "mode-replace", type: "button" },
                      "Replace binding",
                    ),
                    h(
                      "button",
                      { class: "btn", "data-act": "mode-add", type: "button" },
                      "Add another key",
                    ),
                    h(
                      "button",
                      { class: "btn", "data-act": "clear-one", type: "button" },
                      "Clear binding",
                    ),
                    h("button", { class: "btn", "data-act": "cancel", type: "button" }, "Cancel"),
                  ),
                  h(
                    "p",
                    { class: "mhint" },
                    "Delete or Backspace also clears it. “Add another key” makes the ",
                    "next press an EXTRA key for this control — either key then presses ",
                    "it (MAME-style), instead of the new one taking the old one's place.",
                  ),
                  // ── v13: AUTO-FIRE, in the same vocabulary ──────────────
                  // "where is the option to make buttons turbo?" — here, on
                  // the control you just clicked, beside Replace/Add/Clear.
                  // It writes through the SAME map verb with the control's
                  // current keys: turbo is a property of the CONTROL, so
                  // setting it is that control's write with one more field.
                  h(
                    "div",
                    { class: "mturbo" },
                    h(
                      "label",
                      { class: "bindlabel" },
                      "turbo",
                      h("input", {
                        class: "mturboin",
                        type: "number",
                        min: "0",
                        step: "1",
                        placeholder: "Hz",
                        "aria-label": "auto-fire rate in presses per second",
                      }),
                    ),
                    h(
                      "button",
                      { class: "btn", "data-act": "turbo-set", type: "button" },
                      "Set turbo",
                    ),
                    h(
                      "button",
                      { class: "btn", "data-act": "turbo-clear", type: "button" },
                      "No turbo",
                    ),
                  ),
                  h("p", { class: "mhint" }, () => modalTurboLine()),
                ),
            ),
            createShow(
              () => modalConflict(),
              () =>
                h(
                  "div",
                  { class: "mbody" },
                  h("p", { class: "conflict" }, () => conflictLine()),
                  h(
                    "div",
                    { class: "mbtns" },
                    h(
                      "button",
                      { class: "btn btn-primary", "data-act": "replace", type: "button" },
                      "Replace",
                    ),
                    h("button", { class: "btn", "data-act": "cancel", type: "button" }, "Cancel"),
                  ),
                ),
            ),
          ),
        ),
    ),
    // ── FEATURE 2: the multi-select action bar (client-only). Appended LAST
    // in document order on purpose — ledger #14: a show inserted in the middle
    // shifts every show after it, and this one is `position: fixed` anyway. ──
    createShow(
      () => selBar(),
      () =>
        h(
          "div",
          { class: "selbar" },
          h("span", { class: "selcount" }, () => selCountLine()),
          h(
            "button",
            { class: "btn btn-primary", "data-act": "map-selected", type: "button" },
            "Map all to one key",
          ),
          h(
            "button",
            { class: "btn", "data-act": "clear-selected", type: "button" },
            "Clear selected",
          ),
          h(
            "button",
            { class: "btn", "data-act": "cancel-select", type: "button" },
            "Cancel",
          ),
        ),
    ),
    // ── The toast stack (v8): every action's report, and its road back. ───
    // NOT a show — the container is always in the DOM and the LIST inside it
    // is empty until something happens, so this costs zero MAP_SHOW_ORDER
    // entries (ledger #4/#14: a new show is a four-file edit that shifts
    // every show after it). SSR renders the empty list's markers, which is
    // exactly what the adoption path needs to insert into later.
    // The container is `pointer-events: none` so an empty stack cannot eat a
    // click meant for the page; each toast turns them back on.
    h(
      "div",
      { class: "toasts", "aria-live": "polite", "aria-atomic": "false" },
      createList(
        () => toasts(),
        (t) => t.id + "|" + t.cls + "|" + t.text + "|" + t.undocls,
        (t) =>
          h(
            "div",
            { class: t.cls, "data-toast": t.id },
            h("p", { class: "tmsg" }, t.text),
            h(
              "div",
              { class: "tbtns" },
              // The label is a constant, so it is a literal child (no slot);
              // whether the button EXISTS is the per-item class field —
              // ledger #15's hide-by-class-string, never `:empty`.
              h(
                "button",
                { class: t.undocls, "data-undo": t.id, type: "button", title: t.undotitle },
                "Undo",
              ),
              h(
                "button",
                {
                  class: "tclose",
                  "data-dismiss": t.id,
                  type: "button",
                  title: t.dismisstitle,
                  "aria-label": t.dismisstitle,
                },
                "✕",
              ),
            ),
          ),
      ),
    ),
    h(
      "footer",
      null,
      h(
        "p",
        null,
        "Writes go through the daemon pipe's `map` verb (same writer as `ksx map`); ",
        "bindings re-read every 2 s. Generated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Serving 127.0.0.1 only.",
      ),
      h(
        "p",
        null,
        "controller art: ",
        h(
          "a",
          { href: "https://github.com/AL2009man/Gamepad-Asset-Pack" },
          "Gamepad-Asset-Pack (MIT) by AL2009man",
        ),
      ),
    ),
  );
}
