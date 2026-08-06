import { h, createSignal, createList, createShow } from "@getforma/core";

// THE MAPPER (v5): click a control on the pad art, press the panel key,
// binding saved. One island, same architecture as StatusIsland.ts — the
// module-level signals are the page's live state store; `applyMap` seeds
// them from the server props BEFORE adoption (ledger #5) and the 2 s
// /api/map poller (map.ts) keeps rewriting them. Derivations here MIRROR
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

/** What GET /api/map serves and what the island props carry — `MapPayload`
 *  in snapshot.rs; parity pinned there. */
export interface MapPayload {
  mapper: MapperSnapshot;
  session: SessionView;
  learn: LearnView;
  selected: number;
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

interface SlotTab {
  num: string;
  label: string;
  cls: string;
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
  /** "✕" on a bound row of a live page, "" otherwise (CSS hides empty). */
  clear: string;
  cleartitle: string;
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
  ["ly.max", "▲", "dir", 24.0, 31.7, 7.0, 6.0, "chip"],
  ["ly.min", "▼", "dir", 24.0, 47.7, 7.0, 6.0, "chip"],
  ["lx.min", "◀", "dir", 17.25, 39.7, 5.5, 7.0, "chip"],
  ["lx.max", "▶", "dir", 30.75, 39.7, 5.5, 7.0, "chip"],
  ["dpad.up", "▲", "dir", 36.4, 50.6, 7.0, 9.0, "chip"],
  ["dpad.down", "▼", "dir", 36.4, 69.2, 7.0, 9.0, "chip"],
  ["dpad.left", "◀", "dir", 29.2, 59.9, 7.0, 9.0, "chip"],
  ["dpad.right", "▶", "dir", 43.6, 59.9, 7.0, 9.0, "chip"],
  ["rthumb", "R3", "hub", 62.5, 58.4, 8.0, 10.0, "round"],
  ["ry.max", "▲", "dir", 62.5, 50.4, 7.0, 6.0, "chip"],
  ["ry.min", "▼", "dir", 62.5, 66.4, 7.0, 6.0, "chip"],
  ["rx.min", "◀", "dir", 55.75, 58.4, 5.5, 7.0, "chip"],
  ["rx.max", "▶", "dir", 69.25, 58.4, 5.5, 7.0, "chip"],
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
  ["ly.max", "▲", "dir", 33.8, 48.8, 7.0, 6.0, "chip"],
  ["ly.min", "▼", "dir", 33.8, 64.8, 7.0, 6.0, "chip"],
  ["lx.min", "◀", "dir", 27.05, 56.8, 5.5, 7.0, "chip"],
  ["lx.max", "▶", "dir", 40.55, 56.8, 5.5, 7.0, "chip"],
  ["dpad.up", "▲", "dir", 18.5, 31.5, 5.4, 7.2, "chip"],
  ["dpad.down", "▼", "dir", 18.5, 46.6, 5.4, 7.2, "chip"],
  ["dpad.left", "◀", "dir", 12.9, 39.2, 5.4, 7.2, "chip"],
  ["dpad.right", "▶", "dir", 23.9, 39.2, 5.4, 7.2, "chip"],
  ["rthumb", "R3", "hub", 66.1, 56.8, 8.0, 10.0, "round"],
  ["ry.max", "▲", "dir", 66.1, 48.8, 7.0, 6.0, "chip"],
  ["ry.min", "▼", "dir", 66.1, 64.8, 7.0, 6.0, "chip"],
  ["rx.min", "◀", "dir", 59.35, 56.8, 5.5, 7.0, "chip"],
  ["rx.max", "▶", "dir", 72.85, 56.8, 5.5, 7.0, "chip"],
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
const [modalPrompt, setModalPrompt] = createSignal("");
const [modalBinding, setModalBinding] = createSignal("");
const [countdownText, setCountdownText] = createSignal("");
const [barStyle, setBarStyle] = createSignal("width:100%");
const [conflictLine, setConflictLine] = createSignal("");
const [savedLine, setSavedLine] = createSignal("");
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
const [savedOk, setSavedOk] = createSignal(false);
const [savedErr, setSavedErr] = createSignal(false);
const [modalOpen, setModalOpen] = createSignal(false);
const [modalListening, setModalListening] = createSignal(false);
const [modalBound, setModalBound] = createSignal(false);
const [modalConflict, setModalConflict] = createSignal(false);
const [selBar, setSelBar] = createSignal(false);

const [slotTabs, setSlotTabs] = createSignal<SlotTab[]>([]);
const [zones, setZones] = createSignal<ZoneRow[]>([]);
const [legendRows, setLegendRows] = createSignal<LegendRow[]>([]);
/** The preset-actions card's class: "card pactions" when the daemon can
 *  restore, "card pactions off" (inert look, clicks flash the reason) when
 *  not. A class string, not a show — the card never unmounts. */
const [actionsCls, setActionsCls] = createSignal("card pactions off");

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

/** "A", "✕", "D-pad ▲" — how this persona names a control, for prompts. */
export function identityLabel(fn: string): string {
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
  // an action to controls the user is no longer looking at.
  selection.clear();
  setSelBar(false);
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

function keyTag(slot: MapperSlot, fn: string): string {
  const keys = slot.bindings[fn];
  return keys && keys.length > 0 ? keys.join("+") : "—";
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
  const tags = table.map(([fn]) => keyTag(slot, fn));
  return tags.map((tag, i) =>
    tag === "—"
      ? []
      : table
          .filter((_, j) => j !== i && tags[j] === tag)
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
        share.length > 0
          ? `${fn} — ${key} (${shareTitle(key, share)})`
          : `${fn} — ${key}`,
      tag: key === "—" ? "" : key,
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

function legendRowsFor(slot: MapperSlot): LegendRow[] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  const shared = sharedLabels(slot);
  return table.map(([fn, label, idk], i) => {
    const key = keyTag(slot, fn);
    const unbound = key === "—";
    const share = shared[i];
    return {
      fn,
      label: legendLabel(fn, label),
      id: label,
      idcls: `lid id-${idk}`,
      group: legendGroup(fn),
      key,
      cls:
        `lrow${unbound ? " l-unbound" : ""}${liveMapping ? "" : " l-dead"}` +
        `${share.length > 0 ? " l-shared" : ""}${fn === hotFn ? " l-hot" : ""}` +
        `${selection.has(fn) ? " l-sel" : ""}`,
      title: `${fn} — ${key}`,
      share: shareText(share),
      sharetitle: shareTitle(key, share),
      // The desktop accelerator. Only where clearing would do something; the
      // learn modal's "Clear binding" is the primary, touch-first path.
      clear: liveMapping && !unbound ? "✕" : "",
      cleartitle: `clear ${fn}`,
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
      "read-only while emulation runs: the panel's keys are captured, so ksx cannot " +
      "hear them for mapping. Use the Pause button above, or bind from a shell " +
      "with the command below"
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
    })),
  );
  setZones(slot ? zoneRows(slot) : []);
  setLegendRows(slot ? legendRowsFor(slot) : []);
  setSlotLine(
    slot ? `P${slot.number} · ${slot.persona_label} · ${slot.preset}` : "no mappable slots",
  );
  setSourceLine(`${p.mapper.source} — config root: ${p.mapper.config_root}`);
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
  const keys = slot.bindings[fn];
  return keys && keys.length > 0 ? keys.join("+") : null;
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
  setModalBinding(bindingText === null ? "" : `currently ${bindingText}`);
  setModalBound(bindingText !== null);
  setModalOpen(true);
  setModalListening(true);
  setModalConflict(false);
  updateCountdown(remainingMs, totalMs);
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

const FLASH_MS = 5000;
let flashTimer: ReturnType<typeof setTimeout> | undefined;

export function flashSaved(line: string, isError: boolean): void {
  if (flashTimer !== undefined) clearTimeout(flashTimer);
  setSavedLine(line);
  setSavedOk(!isError && line !== "");
  setSavedErr(isError && line !== "");
  if (line !== "") {
    flashTimer = setTimeout(() => flashSaved("", false), FLASH_MS);
  }
}

// ── The screen ─────────────────────────────────────────────────────────────
// createShow document order == render_map.rs MAP_SHOW_ORDER (positional seam,
// ledger #4). Nineteen, in this order:
//   pillRunning, pillIdle, pillDown, pillPaused,
//   noDaemon, sessionRunning, pausedBar, readOnly, canLearn,
//   artXbox, artDs4, hasBackup, savedOk, savedErr,
//   modalOpen, modalListening, modalBound, modalConflict,
//   selBar (v7 — APPENDED, never inserted: ledger #14).
// Adding or reordering one here without updating MAP_SHOW_ORDER shows the
// wrong panel; `embedded_map_ir_slot_layout_matches_the_seam` catches it.

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
        h("span", { class: "crumb" }, "mapper"),
      ),
      h("a", { class: "navlink", href: "/" }, "← Status"),
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
              h(
                "button",
                { class: "btn btn-primary", "data-act": "pause-map", type: "button" },
                "Pause emulation & map",
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
      h(
        "section",
        { class: "card slotstrip" },
        h(
          "div",
          { class: "tabs" },
          createList(
            () => slotTabs(),
            (t) => t.num + "|" + t.label + "|" + t.cls,
            (t) => h("button", { class: t.cls, "data-slot": t.num, type: "button" }, t.label),
          ),
        ),
        h("p", { class: "slotline" }, () => slotLine()),
        h("p", { class: "srcline mono" }, () => sourceLine()),
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
          h(
            "p",
            { class: "hint" },
            "Click a control, then press the panel key for it — Esc or a click ",
            "outside cancels, Delete clears. Ctrl-click (or “Select multiple”) ",
            "picks several controls and maps them all to ONE key. Saves are ",
            "immediate, and a running session takes them live without ",
            "unplugging the pads.",
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
                "button",
                { class: l.cls, "data-fn": l.fn, type: "button", title: l.title },
                // The same identity glyph the art wears, so the two readers
                // are visibly the same control.
                h("span", { class: l.idcls }, l.id),
                h("span", { class: "llabel" }, l.group),
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
              ),
          ),
        ),
      ),
      // ── Preset actions: save semantics + the two restore safety nets.
      // Always rendered (a class string flips the inert look — never a
      // show, so its bindings survive; ledger #13). Buttons share map.ts's
      // data-act delegation; each confirms before the pipe verb. ──────────
      h(
        "section",
        { class: () => actionsCls() },
        h(
          "div",
          { class: "phead" },
          h("h2", null, "Preset"),
          // Auto-save, made visible. Empty until this page writes something.
          h("span", { class: "savedat mono" }, () => savedAt()),
        ),
        h(
          "p",
          { class: "savenote" },
          "Every binding saves immediately — there is no Save button. A running ",
          "session takes binding changes live, without unplugging the pads. Use ",
          "the restore options below to undo.",
        ),
        h(
          "div",
          { class: "pactrow" },
          h(
            "button",
            { class: "btn btn-row", "data-act": "clear-all", type: "button" },
            "Clear all bindings",
          ),
          h(
            "button",
            { class: "btn btn-row", "data-act": "restore-backup", type: "button" },
            "Undo this session",
          ),
          // FIX 2's third destination — only rendered when a backup exists,
          // because an offer of a road home that is not there is worse than
          // no offer. The timestamp is IN the label, not in a tooltip.
          createShow(
            () => hasBackup(),
            () =>
              h(
                "button",
                { class: "btn btn-row", "data-act": "restore-latest", type: "button" },
                () => backupLine(),
              ),
          ),
          // FIX 2: the label names the LAYOUT it writes. "Restore built-in
          // defaults" read, to Victor, as "put my I-PAC map back" — and wrote
          // a desktop-keyboard layout over it.
          h(
            "button",
            { class: "btn btn-row btn-danger-ghost", "data-act": "restore-defaults", type: "button" },
            "Reset to generic keyboard layout (S/D/A/W…)",
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
                  h(
                    "div",
                    { class: "mbtns" },
                    h(
                      "button",
                      { class: "btn", "data-act": "clear-one", type: "button" },
                      "Clear binding",
                    ),
                    h("button", { class: "btn", "data-act": "cancel", type: "button" }, "Cancel"),
                  ),
                  h("p", { class: "mhint" }, "Delete or Backspace also clears it."),
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
