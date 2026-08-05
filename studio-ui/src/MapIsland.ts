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
// PadForge rule: derive layout from art with a script). Zones are PURE HIT
// AREAS — transparent, no inline text (the art is the label; the title
// tooltip names fn + binding); the readable truth is the bindings LEGEND
// below the stage, one row per function, which shares the zone click action
// via the same data-fn delegation. A shared hover signal (`setHot`)
// cross-highlights zone ↔ legend row. Interaction lives in map.ts (event
// delegation, so list reconcile keeps everything wired).

// ── Wire types: serde field names from ksx-studio {snapshot,control}.rs ────

export interface MapperSlot {
  number: number;
  persona: string;
  persona_label: string;
  preset: string;
  keyboard: string;
  bindings: Record<string, string[]>;
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
  style: string;
  title: string;
  /// The on-zone binding tag ("" for unbound — CSS hides the empty pill).
  tag: string;
}

interface LegendRow {
  fn: string;
  label: string;
  key: string;
  cls: string;
  title: string;
}

// ── Zone tables — MIRROR of render_map.rs ZONE_XBOX / ZONE_DS4 ────────────
// [fn, label, cx, cy, w, h, kind]; stage-percent boxes, art bottom-aligned
// at 86% stage height (ART_SHARE). Rects are pairwise DISJOINT (pinned by
// render_map.rs `zone_tables_cover_every_mappable_function`): face buttons
// sized to the drawn circles, dpad arrows to the drawn cross, and the four
// stick-direction wedges RING the stick with the L3/R3 click zone as the
// center hub — adjacent, never covering it.

type ZoneDef = [string, string, number, number, number, number, string];

const ZONE_XBOX: ZoneDef[] = [
  ["lt", "LT", 31.0, 4.6, 10.0, 5.2, "trigger"],
  ["lb", "LB", 34.0, 10.9, 11.0, 5.2, "bumper"],
  ["rb", "RB", 66.0, 10.9, 11.0, 5.2, "bumper"],
  ["rt", "RT", 69.0, 4.6, 10.0, 5.2, "trigger"],
  ["Y", "Y", 75.2, 31.1, 7.2, 8.4, "round"],
  ["B", "B", 82.0, 39.6, 7.2, 8.4, "round"],
  ["A", "A", 75.3, 48.3, 7.2, 8.4, "round"],
  ["X", "X", 68.7, 39.7, 7.2, 8.4, "round"],
  ["guide", "guide", 50.0, 27.0, 9.0, 11.0, "round"],
  ["back", "view", 44.0, 39.0, 6.5, 8.0, "chip"],
  ["start", "menu", 56.0, 39.0, 6.5, 8.0, "chip"],
  ["lthumb", "L3", 24.0, 39.7, 8.0, 10.0, "round"],
  ["ly.max", "▲", 24.0, 31.7, 7.0, 6.0, "chip"],
  ["ly.min", "▼", 24.0, 47.7, 7.0, 6.0, "chip"],
  ["lx.min", "◀", 17.25, 39.7, 5.5, 7.0, "chip"],
  ["lx.max", "▶", 30.75, 39.7, 5.5, 7.0, "chip"],
  ["dpad.up", "▲", 36.4, 50.6, 7.0, 9.0, "chip"],
  ["dpad.down", "▼", 36.4, 69.2, 7.0, 9.0, "chip"],
  ["dpad.left", "◀", 29.2, 59.9, 7.0, 9.0, "chip"],
  ["dpad.right", "▶", 43.6, 59.9, 7.0, 9.0, "chip"],
  ["rthumb", "R3", 62.5, 58.4, 8.0, 10.0, "round"],
  ["ry.max", "▲", 62.5, 50.4, 7.0, 6.0, "chip"],
  ["ry.min", "▼", 62.5, 66.4, 7.0, 6.0, "chip"],
  ["rx.min", "◀", 55.75, 58.4, 5.5, 7.0, "chip"],
  ["rx.max", "▶", 69.25, 58.4, 5.5, 7.0, "chip"],
];

const ZONE_DS4: ZoneDef[] = [
  ["lt", "L2", 17.0, 4.6, 9.5, 5.2, "trigger"],
  ["lb", "L1", 19.5, 10.9, 10.5, 5.2, "bumper"],
  ["rb", "R1", 80.5, 10.9, 10.5, 5.2, "bumper"],
  ["rt", "R2", 83.0, 4.6, 9.5, 5.2, "trigger"],
  ["Y", "△", 81.2, 29.2, 7.0, 9.0, "round"],
  ["B", "○", 88.4, 38.8, 7.0, 9.0, "round"],
  ["A", "✕", 81.3, 48.1, 7.0, 9.0, "round"],
  ["X", "□", 74.0, 38.7, 7.0, 9.0, "round"],
  ["back", "share", 30.0, 25.5, 7.0, 9.0, "chip"],
  ["start", "options", 70.0, 25.5, 7.0, 9.0, "chip"],
  ["guide", "PS", 50.0, 63.0, 8.0, 10.0, "round"],
  ["lthumb", "L3", 33.8, 56.8, 8.0, 10.0, "round"],
  ["ly.max", "▲", 33.8, 48.8, 7.0, 6.0, "chip"],
  ["ly.min", "▼", 33.8, 64.8, 7.0, 6.0, "chip"],
  ["lx.min", "◀", 27.05, 56.8, 5.5, 7.0, "chip"],
  ["lx.max", "▶", 40.55, 56.8, 5.5, 7.0, "chip"],
  ["dpad.up", "▲", 18.5, 31.5, 5.4, 7.2, "chip"],
  ["dpad.down", "▼", 18.5, 46.6, 5.4, 7.2, "chip"],
  ["dpad.left", "◀", 12.9, 39.2, 5.4, 7.2, "chip"],
  ["dpad.right", "▶", 23.9, 39.2, 5.4, 7.2, "chip"],
  ["rthumb", "R3", 66.1, 56.8, 8.0, 10.0, "round"],
  ["ry.max", "▲", 66.1, 48.8, 7.0, 6.0, "chip"],
  ["ry.min", "▼", 66.1, 64.8, 7.0, 6.0, "chip"],
  ["rx.min", "◀", 59.35, 56.8, 5.5, 7.0, "chip"],
  ["rx.max", "▶", 72.85, 56.8, 5.5, 7.0, "chip"],
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
const [modalPrompt, setModalPrompt] = createSignal("");
const [countdownText, setCountdownText] = createSignal("");
const [barStyle, setBarStyle] = createSignal("width:100%");
const [conflictLine, setConflictLine] = createSignal("");
const [savedLine, setSavedLine] = createSignal("");
const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [readOnly, setReadOnly] = createSignal(false);
const [canLearn, setCanLearn] = createSignal(false);
const [artXbox, setArtXbox] = createSignal(false);
const [artDs4, setArtDs4] = createSignal(false);
const [savedOk, setSavedOk] = createSignal(false);
const [savedErr, setSavedErr] = createSignal(false);
const [modalOpen, setModalOpen] = createSignal(false);
const [modalListening, setModalListening] = createSignal(false);
const [modalConflict, setModalConflict] = createSignal(false);

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

export function setHot(fn: string | null): void {
  if (hotFn === fn) return;
  hotFn = fn;
  const slot = currentSlot();
  if (slot) {
    setZones(zoneRows(slot));
    setLegendRows(legendRowsFor(slot));
  }
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

function zoneRows(slot: MapperSlot): ZoneRow[] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  return table.map(([fn, , cx, cy, w, h, kind]) => {
    const key = keyTag(slot, fn);
    // z-unbound hides the tag pill via CSS: `:empty` cannot work, the SSR
    // text slot leaves marker nodes inside the span.
    return {
      fn,
      cls: `zone z-${kind}${key === "—" ? " z-unbound" : ""}${fn === hotFn ? " z-hot" : ""}`,
      style:
        `left:${(cx - w / 2).toFixed(1)}%;top:${(cy - h / 2).toFixed(1)}%;` +
        `width:${w.toFixed(1)}%;height:${h.toFixed(1)}%`,
      title: `${fn} — ${key}`,
      tag: key === "—" ? "" : key,
    };
  });
}

/** "LS ▲", "D-pad ◀", "✕" — the legend's control label, persona-aware and
 *  unambiguous once the four stick/dpad glyph groups are prefixed. */
function legendLabel(fn: string, label: string): string {
  if (fn.startsWith("lx.") || fn.startsWith("ly.")) return `LS ${label}`;
  if (fn.startsWith("rx.") || fn.startsWith("ry.")) return `RS ${label}`;
  if (fn.startsWith("dpad.")) return `D-pad ${label}`;
  return label;
}

function legendRowsFor(slot: MapperSlot): LegendRow[] {
  const table = isPlaystation(slot.persona) ? ZONE_DS4 : ZONE_XBOX;
  return table.map(([fn, label]) => {
    const key = keyTag(slot, fn);
    const unbound = key === "—";
    return {
      fn,
      label: legendLabel(fn, label),
      key,
      cls: `lrow${unbound ? " l-unbound" : ""}${fn === hotFn ? " l-hot" : ""}`,
      title: `${fn} — ${key}`,
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
      "read-only while a session is running: captured keys never reach the learner. " +
      "Stop the session to map here, or bind from a shell with the command below (then Reload)"
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

  const live = learnable(p) && slot !== null;
  setReasonLine(reason(p));
  setReadOnly(!live);
  setCanLearn(live);
  setActionsCls(p.session.reachable ? "card pactions" : "card pactions off");
  setArtXbox(slot !== null && !isPlaystation(slot.persona));
  setArtDs4(slot !== null && isPlaystation(slot.persona));

  setPillRunning(p.session.reachable && p.session.running);
  setPillIdle(p.session.reachable && !p.session.running);
  setPillDown(!p.session.reachable);

  refreshCliLine();
}

/** The studio server itself stopped answering: keep the page, say so. */
export function applyMapUnreachable(): void {
  setReasonLine("ksx-studio not responding — retrying every 2 s");
  setReadOnly(true);
  setCanLearn(false);
  setActionsCls("card pactions off");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
}

// ── Modal + flash state (driven by map.ts) ─────────────────────────────────

export function showListening(promptText: string, remainingMs: number, totalMs: number): void {
  setModalPrompt(promptText);
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
  setModalConflict(true);
  setConflictLine(line);
}

export function closeModal(): void {
  setModalOpen(false);
  setModalListening(false);
  setModalConflict(false);
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
// createShow document order == render_map.rs MAP_SHOW_ORDER (positional
// seam, ledger #4): pills ×3, readOnly, canLearn, artXbox, artDs4, savedOk,
// savedErr, modalOpen, modalListening, modalConflict.

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
    ),
    h(
      "main",
      null,
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
            "outside cancels. Saved bindings hot-reload a running session via a ",
            "clean daemon Reload.",
          ),
      ),
      // ── THE CONTROLLER (huge). Art + zone layer per persona. ──────────
      h(
        "section",
        { class: "card stagecard" },
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
                  (z) => z.fn + "|" + z.cls + "|" + z.style + "|" + z.title,
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
                  (z) => z.fn + "|" + z.cls + "|" + z.style + "|" + z.title,
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
            (l) => l.fn + "|" + l.label + "|" + l.key + "|" + l.cls,
            (l) =>
              h(
                "button",
                { class: l.cls, "data-fn": l.fn, type: "button", title: l.title },
                h("span", { class: "llabel" }, l.label),
                h("span", { class: "lkey" }, l.key),
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
        h("h2", null, "Preset"),
        h(
          "p",
          { class: "savenote" },
          "Bindings save to the preset file immediately — there is no separate ",
          "Save. A running session hot-reloads on each save. Restore points: the ",
          "built-in default layout, or the session-start backup taken before this ",
          "daemon session's first change.",
        ),
        h(
          "div",
          { class: "pactrow" },
          h(
            "button",
            { class: "btn btn-row", "data-act": "restore-backup", type: "button" },
            "Undo this session",
          ),
          h(
            "button",
            { class: "btn btn-row", "data-act": "restore-defaults", type: "button" },
            "Restore built-in defaults",
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
