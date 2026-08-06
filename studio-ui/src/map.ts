import { activateIslands, createUnownedRoot, untrack } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// Dogfood ledger #13 (docs/FORMA-DOGFOOD.md): the adoption-path show effect
// materializes re-toggled branches INSIDE its own reactive run, so every
// binding created there is disposed when the effect re-runs — stale modal
// prompts, empty flash boxes, dead conflict dialogs. build.mjs patches the
// compiled setupShowEffect to route branch creation through this unowned
// root; installed at module top so it exists before any island activates.
(globalThis as unknown as Record<string, unknown>).__ksxShowBranch = (make: () => unknown) =>
  createUnownedRoot(() => untrack(make));
// Compile-time anchor: the imported *Page component NOT in the
// activateIslands registry is this entry's SSR root (see status.ts).
import { MapPage } from "./MapPage";
import {
  MapIsland,
  applyMap,
  applyMapUnreachable,
  blockedReason,
  clearPaused,
  clearSelection,
  closeModal,
  currentBinding,
  currentPreset,
  currentSlot,
  dismissToast,
  holdToasts,
  identityLabel,
  isMultiMode,
  isPaused,
  keyList,
  learnAllowed,
  liveProfile,
  macroAskAboutShortSteps,
  macroClearShortStepQuestion,
  macroDraftTriggers,
  macroInsertMotion,
  macroIsDirty,
  macroIsOnDisk,
  macroNameProblem,
  macroOnDiskCopy,
  macroShortStepQuestion,
  macroSeededFrom,
  macroSetAllowShort,
  macroSetDurationAt,
  macroRateUnit,
  macroRepeatValue,
  macroRowDuration,
  macroSetPolicy,
  macroSetTurboRate,
  macroStepAllowShort,
  macroStepVerb,
  macroTargetRate,
  macroToggleCell,
  macroTomlText,
  macroTurboBoxValue,
  markMacroSaved,
  newMacroBody,
  setMacroTargetRate,
  markPaused,
  markSaved,
  modalIsOpen,
  newestUndoable,
  previousKeys,
  currentMacro,
  seedMacro,
  profileToResume,
  pushToast,
  releaseToasts,
  replaceToast,
  runUndo,
  selectFn,
  selectSlot,
  selectedFnName,
  selectedFns,
  selectionCount,
  setHot,
  setMacroEditorFocused,
  setMultiMode,
  showConflict,
  showLearnMode,
  showLearnTurbo,
  showListening,
  toggleSelected,
  turboHzOf,
  updateCountdown,
  writableKeys,
  type BindOutcome,
  type LearnView,
  type MacroOutcome,
  type MacroView,
  type MapPayload,
  type ToastOptions,
} from "./MapIsland";

void MapPage; // compile-time anchor only

/** Bindings/session poll cadence — same as the status page. */
const POLL_MS = 2000;
/** While learning: poll the daemon's learner at PadForge's recorder tick
 *  (33 ms, docs/research/padforge-code-audit.md §1.2) — it doubles as the
 *  smooth countdown update, the visible timer PadForge never had. */
const LEARN_POLL_MS = 33;
/** The daemon's learn timeout (LEARN_TIMEOUT in daemon/learn.rs). */
const LEARN_TOTAL_MS = 10_000;

type Json = Record<string, unknown>;

interface VerbOutcome {
  ok: boolean;
  message: string | null;
  error: string | null;
}

async function poll(): Promise<void> {
  try {
    // The SELECTED slot travels with the poll. `MapPayload.macros` is ONE
    // preset's table — the selected slot's (server.rs `collect_map`) — and a
    // request with no `slot=` makes the server fall back to the FIRST slot. So
    // on /map?slot=2 the SSR paint and the hydration seed were right and then,
    // two seconds later, the macro card silently swapped to P1's macros while
    // the slot rail, the legend and the stage all still said P2. Saving from
    // that state writes P1's steps into P2's preset, because `saveMacro`
    // resolves the preset from the CLIENT's selection — which never moved.
    // Everything else in the payload is slot-independent, so this is the only
    // parameter the poller needs.
    const slot = currentSlot();
    const url = slot ? `/api/map?slot=${encodeURIComponent(String(slot.number))}` : "/api/map";
    applyMap(await fetchJSON<MapPayload>(url));
  } catch {
    applyMapUnreachable();
  }
  syncMacroControls();
}

/** One JSON verb → its outcome, with transport failure folded into the same
 *  shape so no caller can forget to handle it. Never throws. */
async function verb(path: string, body?: Json): Promise<VerbOutcome> {
  try {
    return await fetchJSON<VerbOutcome>(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body ?? {}),
    });
  } catch {
    return {
      ok: false,
      message: null,
      error: "request failed — is ksx studio still running?",
    };
  }
}

// ── Toast helpers ──────────────────────────────────────────────────────────
// v8: no action on this page asks "are you sure?" any more. It happens, and
// the toast that reports it carries the way back (MapIsland.ts's stack has
// the full rationale). Undo is COMPOSED from verbs that already exist — no
// new daemon surface — and is only ever offered when it can be honest:
//   • a binding edit  → `/api/bind` with the key we remembered before writing
//   • a whole-preset write → `/api/preset/restore latest-backup`, because
//     clear-all and all three restores snapshot a timestamped .bak BEFORE
//     they write, so the newest backup is the pre-action state.

function oops(text: string): string {
  return pushToast(text, { kind: "err" });
}

/** The daemon's chord advisory, sharpened.
 *
 *  `ksx map` reports a chord whose constituent key is ALSO bound on its own
 *  as "…the game sees X for a moment before the chord completes". Read
 *  quickly that sounds cosmetic — a flicker. It is not: ksx never defers
 *  input, so that moment is a REAL press delivered to the game, and a game
 *  acts on real presses. In a fighting game the light punch actually comes
 *  out before the chord lands. The mapper says so in those words wherever it
 *  surfaces the advisory. */
const CHORD_FLASH_MARK = "before the chord completes";
const CHORD_FLASH_RISK =
  " That is not a cosmetic flicker: ksx does not defer input, so the game " +
  "receives that press and can act on it — the light punch comes out before " +
  "the chord lands.";

/** The daemon's own note (from "note:" on) plus the risk framing, or "". */
function chordAdvisory(message: string | null): string {
  if (!message || !message.includes(CHORD_FLASH_MARK)) return "";
  const at = message.indexOf("note:");
  const note = at >= 0 ? message.slice(at + "note:".length).trim() : message;
  return ` Heads up: ${note}.${CHORD_FLASH_RISK}`;
}

/** Put one control back on the exact key LIST it held before — the undo of
 *  any single-control edit (replace, add, remove-one, clear), through the one
 *  writer that takes a whole set. Whether the offer is made at all is
 *  [`writableKeys`]'s call, made before the toast is pushed. */
function undoOneBinding(
  preset: string,
  fn: string,
  keys: string[],
): () => Promise<string | null> {
  return async () => {
    const outcome = await bindKeys(preset, fn, keys, true);
    if (!outcome.ok) {
      return outcome.error ?? outcome.code ?? "the daemon refused the write";
    }
    markSaved();
    await poll();
    return null;
  };
}

/** The sentence a toast adds when the edit LANDED but cannot be taken back —
 *  because putting the control's old keys back would need a multi-key write
 *  this daemon has no shape for. Named once: it is the only honest reason an
 *  Undo button is missing from a successful single-control edit. */
const NO_UNDO_MULTI =
  " (Putting several keys back is one write this daemon's map verb cannot make, " +
  "so there is no Undo here — the Preset card's restore options are the way back.)";

/** The toast options for a single-control edit: an Undo when the previous key
 *  list can honestly be restored, and nothing pretending otherwise when it
 *  cannot. */
function undoOptions(
  preset: string,
  fn: string,
  before: string[],
  name: string,
): ToastOptions {
  if (!writableKeys(before)) return {};
  return {
    undo: undoOneBinding(preset, fn, before),
    undone:
      before.length === 0
        ? `${name} is unbound again.`
        : `${name} is back on ${keyList(before)}.`,
  };
}

/** The same, for a whole group: every affected control back to its own
 *  previous key, in one batch. Partial failure is REPORTED, never swallowed. */
function undoGroup(
  preset: string,
  before: Map<string, string[]>,
): () => Promise<string | null> {
  return async () => {
    const failed: string[] = [];
    for (const [fn, keys] of before) {
      const outcome = await bindKeys(preset, fn, keys, true);
      if (!outcome.ok) {
        failed.push(`${identityLabel(fn)} (${outcome.error ?? outcome.code ?? "refused"})`);
      }
    }
    markSaved();
    await poll();
    return failed.length === 0 ? null : `could not put back ${failed.join("; ")}`;
  };
}

/** A group can only be undone if every control in it can be written back as
 *  it was ([`writableKeys`]) — otherwise one member would come back with half
 *  its keys, and a half-true Undo is worse than none. */
function groupUndoable(before: Map<string, string[]>): boolean {
  return Array.from(before.values()).every(writableKeys);
}

/** The undo of a whole-preset write: restore the backup it just took. Only
 *  offered when a backup is actually on disk (checked against the poll that
 *  follows the action) — an Undo that might lie is not offered at all. */
function undoFromBackup(preset: string): () => Promise<string | null> {
  return async () => {
    const out = await verb("/api/preset/restore", { preset, mode: "latest-backup" });
    if (!out.ok) return out.error ?? "the daemon refused the restore";
    markSaved();
    await poll();
    return null;
  };
}

// ── The learn flow ─────────────────────────────────────────────────────────
// click zone → POST /api/learn/start → poll GET /api/learn until hit /
// timeout / cancelled → on hit POST /api/bind (conflict → Replace re-POSTs
// with force) → flash the outcome → immediate /api/map refresh.
//
// v7: the same flow serves ONE control or MANY. A multi-select arm captures a
// single key press and writes it to every selected control — N ordinary `map`
// calls, which is all a multi-bind is (docs/INPUT-TRANSFORMS.md §1a).

/** What the armed learn will write to. Empty = nothing armed, one entry = the
 *  single rebind, several = "map all to one key". */
let learnTargets: string[] = [];
/** Supersede guard. The single-fn flow could compare `learningFn` by value;
 *  a list cannot, so every arm bumps a generation and late polls check it. */
let learnGen = 0;
let learnTimer: number | undefined;
/** The hit waiting on the conflict dialog's verdict. */
let pendingKey: string | null = null;
/** What the next captured key DOES to a control that already has one.
 *
 *  "replace" is the default on every arm, deliberately: it is what every
 *  mapper in the field study does on a rebind, it is what a user who clicked a
 *  bound control almost always means, and it is the one that always has a
 *  wire shape. "add" is chosen per-capture in the modal and reverts the moment
 *  the modal closes — a sticky mode would silently turn the NEXT rebind into a
 *  fan-out nobody asked for. */
let learnMode: "replace" | "add" = "replace";

function learning(): boolean {
  return learnTargets.length > 0;
}

function prompt(fn: string): string {
  const slot = currentSlot();
  return slot ? `Press the panel key for P${slot.number} · ${fn}` : `Press the panel key for ${fn}`;
}

/** "both" / "all three" / "all 12" — the multi prompt says what the press will
 *  DO, in words, before it happens. */
function allOf(n: number): string {
  const words = ["", "", "both", "all three", "all four", "all five", "all six"];
  return words[n] ?? `all ${n}`;
}

/** FEATURE 2's prompt: names every selected control by its identity on THIS
 *  persona, and states the outcome plainly (MAPPER-UX commandment 6). */
function multiPrompt(fns: string[]): string {
  const slot = currentSlot();
  const who = slot ? `P${slot.number} · ` : "";
  return (
    `Press the panel key for ${who}${fns.map(identityLabel).join(", ")}` +
    ` — one key will drive ${allOf(fns.length)}.`
  );
}

function stopLearnTimer(): void {
  if (learnTimer !== undefined) {
    window.clearInterval(learnTimer);
    learnTimer = undefined;
  }
}

// ── Browser-focus guard ────────────────────────────────────────────────────
// While a learn is armed the session is stopped, so the panel's keys reach
// Windows — and therefore this page. Space or Enter would then "click" whatever
// element has focus (the zone button that armed the learn, most likely), and a
// letter key would type into anything focusable. Neither is what the user is
// doing: they are pressing a panel button so the DAEMON can hear it.
//
// So while armed: drop focus, and swallow key events at the capture phase.
// Escape is never swallowed (it cancels); Delete/Backspace are not swallowed
// either — they are the modal's Clear accelerator, handled below.
function guardKeys(ev: KeyboardEvent): void {
  if (!learning()) return;
  if (ev.key === "Escape" || ev.key === "Delete" || ev.key === "Backspace") return;
  ev.preventDefault();
  ev.stopPropagation();
}

function armFocusGuard(): void {
  const active = document.activeElement;
  if (active instanceof HTMLElement) active.blur();
  window.addEventListener("keydown", guardKeys, true);
  window.addEventListener("keypress", guardKeys, true);
}

function disarmFocusGuard(): void {
  window.removeEventListener("keydown", guardKeys, true);
  window.removeEventListener("keypress", guardKeys, true);
}

async function startLearn(fns: string[]): Promise<void> {
  if (fns.length === 0) return;
  // PadForge convention: clicking the control being recorded cancels it.
  if (learnTargets.length === 1 && fns.length === 1 && learnTargets[0] === fns[0]) {
    await cancelLearn();
    return;
  }
  learnTargets = fns;
  const gen = ++learnGen;
  pendingKey = null;
  learnMode = "replace";
  selectFn(fns[0]);
  armFocusGuard();
  try {
    const started = await fetchJSON<LearnView>("/api/learn/start", { method: "POST" });
    if (learnGen !== gen) return; // superseded while the POST was in flight
    if (started.state !== "listening") {
      learnTargets = [];
      disarmFocusGuard();
      oops(`Can't listen for a key: ${started.error ?? `learn refused (${started.state})`}.`);
      return;
    }
    const single = fns.length === 1;
    showListening(
      single ? prompt(fns[0]) : multiPrompt(fns),
      // "currently X" + Clear only makes sense for one control; a multi arm
      // has N current bindings and its own Clear lives in the selection bar.
      single ? currentBinding(fns[0]) : null,
      started.remaining_ms ?? LEARN_TOTAL_MS,
      LEARN_TOTAL_MS,
    );
    // AUTO-FIRE lives beside Replace/Add/Clear, so the modal has to say what
    // this control does today before anyone types a number into the box. A
    // multi-control arm has N rates and no single answer, so it says nothing.
    showLearnTurbo(single ? fns[0] : null);
    const box = islandRoot?.querySelector<HTMLInputElement>(".mturboin");
    if (box) {
      const slot = currentSlot();
      box.value = single && slot ? String(turboHzOf(slot, fns[0]) ?? "") : "";
    }
    stopLearnTimer();
    learnTimer = window.setInterval(() => void pollLearn(), LEARN_POLL_MS);
  } catch {
    learnTargets = [];
    disarmFocusGuard();
    oops("Can't listen for a key: the request failed — is ksx studio still running?");
  }
}

async function pollLearn(): Promise<void> {
  const targets = learnTargets;
  const gen = learnGen;
  if (targets.length === 0) {
    stopLearnTimer();
    return;
  }
  let learn: LearnView;
  try {
    learn = await fetchJSON<LearnView>("/api/learn");
  } catch {
    return; // transient — keep the countdown running on the last known value
  }
  if (learnGen !== gen) return; // superseded meanwhile
  const names = targets.map(identityLabel).join(", ");
  switch (learn.state) {
    case "listening":
      updateCountdown(learn.remaining_ms ?? 0, LEARN_TOTAL_MS);
      break;
    case "hit":
      stopLearnTimer();
      learnTargets = [];
      disarmFocusGuard();
      closeModal();
      if (learn.key) {
        if (targets.length !== 1) await mapAll(targets, learn.key);
        // The modal's own choice: join the control's key list, or take it over.
        else if (learnMode === "add") await addKey(targets[0], learn.key);
        else await saveBinding(targets[0], learn.key, false);
      }
      break;
    case "timeout":
      stopLearnTimer();
      learnTargets = [];
      disarmFocusGuard();
      closeModal();
      oops(`Timed out: no key was pressed within 10 s for ${names}. Nothing changed.`);
      break;
    case "cancelled":
      stopLearnTimer();
      learnTargets = [];
      disarmFocusGuard();
      closeModal();
      break;
    default:
      // failed / unavailable / idle-after-restart: report and stop.
      stopLearnTimer();
      learnTargets = [];
      disarmFocusGuard();
      closeModal();
      oops(`The learner stopped: ${learn.error ?? `learn ${learn.state}`}. Nothing changed.`);
      break;
  }
}

async function cancelLearn(): Promise<void> {
  stopLearnTimer();
  learnTargets = [];
  learnGen += 1;
  pendingKey = null;
  learnMode = "replace";
  disarmFocusGuard();
  closeModal();
  try {
    await fetch("/api/learn/cancel", { method: "POST" });
  } catch {
    // the modal is already closed; a lost cancel just times out server-side
  }
}

/** One `map` write. Transport failure is folded into the same shape so no
 *  caller can forget it — the multi-write loop below depends on that. */
async function bindOnce(
  preset: string,
  fn: string,
  key: string | null,
  force: boolean,
): Promise<BindOutcome> {
  try {
    return await fetchJSON<BindOutcome>("/api/bind", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        preset,
        function: fn,
        key,
        force,
        // A binding-only edit is hot-swapped into a running session: the pads
        // stay plugged (crates/ksx-app/src/daemon/mod.rs `apply_bindings`).
        reload: true,
      }),
    });
  } catch {
    return {
      ok: false,
      message: null,
      error: "bind request failed — is ksx studio still running?",
      code: null,
      conflicts: [],
      reloaded: false,
    };
  }
}

/** Write a control's WHOLE key list — MANY KEYS → ONE CONTROL.
 *
 *  Read-modify-write, computed HERE from the payload the page is already
 *  polling: add = current ∪ {k}, per-key ✕ = current ∖ {k}, undo = whatever
 *  the list was before. The server's `/api/bind/keys` hands the set to
 *  `ControlSource::bind_keys`, the one place that knows how to spell it on the
 *  wire — today by composing the same single-key `map` verb, which is why a
 *  set of two or more comes back as an honest refusal instead of a write that
 *  drops keys. Same never-throws shape as [`bindOnce`]. */
async function bindKeys(
  preset: string,
  fn: string,
  keys: string[],
  force: boolean,
  turboHz?: number,
): Promise<BindOutcome> {
  try {
    return await fetchJSON<BindOutcome>("/api/bind/keys", {
      method: "POST",
      headers: { "content-type": "application/json" },
      // AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3): OMITTED means "not asked
      // about", which is what leaves an existing rate alone — so "Add another
      // key" cannot silently switch a control's auto-fire off. 0 clears it.
      body: JSON.stringify({
        preset,
        function: fn,
        keys,
        force,
        reload: true,
        ...(turboHz === undefined ? {} : { turbo_hz: turboHz }),
      }),
    });
  } catch {
    return {
      ok: false,
      message: null,
      error: "bind request failed — is ksx studio still running?",
      code: null,
      conflicts: [],
      reloaded: false,
    };
  }
}

/** The number typed into the learn modal's turbo box, or `null` for a blank
 *  one (which is a refusal, never a silent clear — “No turbo” is the clear). */
function modalTurboInput(): number | null {
  const raw = islandRoot?.querySelector<HTMLInputElement>(".mturboin")?.value.trim() ?? "";
  if (raw === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) && n >= 0 ? Math.round(n) : null;
}

/** Set (or clear) a control's AUTO-FIRE rate. Same toast + Undo shape as every
 *  other write on this page: the Undo restores the rate that was there before,
 *  because a rate is a binding property and losing one to a mis-tap must be as
 *  recoverable as losing a key. */
async function setTurbo(fn: string, hz: number | null): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  if (hz === null) {
    pushToast(
      "Type a number of presses a second first — or press “No turbo” to switch it off.",
      { kind: "warn" },
    );
    return;
  }
  const name = identityLabel(fn);
  const keys = previousKeys(fn);
  if (keys.length === 0 && hz > 0) {
    pushToast(`${name} has no keys, so there is nothing to auto-fire — bind one first.`, {
      kind: "warn",
    });
    return;
  }
  const before = turboHzOf(slot, fn) ?? 0;
  const outcome = await bindKeys(slot.preset, fn, keys, false, hz);
  if (!outcome.ok) {
    pushToast(outcome.error ?? `could not set turbo on ${name}`, { kind: "error" });
    return;
  }
  const effective = outcome.turbo_effective_hz ?? null;
  const line =
    hz === 0
      ? `${name} no longer auto-fires.`
      : effective !== null && effective !== hz
        ? `${name} auto-fires at about ${effective} Hz — ${hz} Hz was asked for, but a press ` +
          "AND a release must each survive a 60 Hz poll."
        : `${name} auto-fires at ${hz} Hz.`;
  pushToast(line, {
    kind: "ok",
    undo: async () => {
      await bindKeys(slot.preset, fn, keys, false, before);
      await poll();
    },
  });
  await poll();
  showLearnTurbo(fn);
}

/** ADD a key to what a control already holds, instead of replacing it — the
 *  MAME-style OR-chain (`A = ["S", "Enter"]`, press either;
 *  docs/INPUT-TRANSFORMS.md §1a). `force` is true for the same reason the
 *  multi-select arm forces: this is a DELIBERATE fan-out, and force removes
 *  nothing from anybody — it only says yes to a cross-slot duplicate. */
async function addKey(fn: string, key: string): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  const before = previousKeys(fn);
  const name = identityLabel(fn);
  if (before.some((k) => k.toLowerCase() === key.toLowerCase())) {
    pushToast(`${name} already has ${key} — nothing to add.`, { kind: "warn" });
    void poll();
    return;
  }
  const after = [...before, key];
  const outcome = await bindKeys(slot.preset, fn, after, true);
  if (!outcome.ok) {
    oops(`${name} was not changed: ${outcome.error ?? "the daemon refused the write"}`);
    void poll();
    return;
  }
  markSaved();
  const opts = undoOptions(slot.preset, fn, before, name);
  let line =
    after.length > 1
      ? `${name} now has ${keyList(after)} — any one of them presses it.`
      : `${name} is now ${key}.`;
  if (isPaused()) line += " Resume emulation when you're done.";
  if (!opts.undo) line += NO_UNDO_MULTI;
  pushToast(line, opts);
  void poll();
}

/** Remove ONE key from a control and leave the others — the legend chips' ✕.
 *  Taking the last one is an ordinary clear, said in those words. */
async function removeKey(fn: string, key: string): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  const before = previousKeys(fn);
  const after = before.filter((k) => k.toLowerCase() !== key.toLowerCase());
  const name = identityLabel(fn);
  if (after.length === before.length) {
    pushToast(
      `${name} is not bound to ${key}${before.length > 0 ? ` (it has ${keyList(before)})` : ""}.`,
      { kind: "warn" },
    );
    void poll();
    return;
  }
  const outcome = await bindKeys(slot.preset, fn, after, false);
  if (!outcome.ok) {
    oops(`${name} was not changed: ${outcome.error ?? "the daemon refused the write"}`);
    void poll();
    return;
  }
  markSaved();
  const opts = undoOptions(slot.preset, fn, before, name);
  let line =
    after.length > 0
      ? `${key} removed — ${name} still has ${keyList(after)}.`
      : `${key} removed — ${name} is now unbound.`;
  if (isPaused()) line += " Resume emulation when you're done.";
  if (!opts.undo) line += NO_UNDO_MULTI;
  pushToast(line, opts);
  void poll();
}

/** Write one binding — `key: null` CLEARS it (the `ksx map --clear` verb, same
 *  writer, no GUI-only path). */
async function saveBinding(fn: string, key: string | null, force: boolean): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  // Remembered BEFORE the write — this is the entire undo. (Re-entry through
  // the conflict retry below re-reads it, which is still pre-write.)
  const before = previousKeys(fn);
  const was = keyList(before);
  const name = identityLabel(fn);
  const outcome = await bindOnce(slot.preset, fn, key, force);
  if (outcome.ok) {
    closeModal();
    pendingKey = null;
    markSaved();
    let line =
      key === null
        ? `${name} is now unbound${was ? ` — it was ${was}` : ""}.`
        : `${name} is now ${key}${was ? ` — it was ${was}` : ""}.`;
    if (isPaused()) line += " Resume emulation when you're done.";
    line += chordAdvisory(outcome.message);
    // Undo needs to put back exactly what was there — the whole key LIST, not
    // its first entry ([`writableKeys`] is what decides whether that write
    // exists). A write that changed nothing has nothing to undo either.
    const changed = was !== (key ?? "");
    const opts = changed ? undoOptions(slot.preset, fn, before, name) : {};
    if (changed && !opts.undo && before.length > 1) line += NO_UNDO_MULTI;
    pushToast(line, {
      ...opts,
      kind: outcome.message?.includes(CHORD_FLASH_MARK) ? "warn" : "ok",
    });
  } else if (outcome.code === "conflict" && outcome.conflicts.length > 0) {
    // FEATURE 3. A key already used by ANOTHER CONTROL IN THIS PRESET is a
    // multi-bind, not an error: the engine compiles one key to several targets
    // and applies them all (docs/INPUT-TRANSFORMS.md §1a). A current daemon
    // never refuses one — it writes it and reports `also_drives` — so this arm
    // is only reached by an OLD daemon that still calls it a conflict; retry
    // once so the write lands there too. Either way the legend's "also …"
    // badges re-derive from disk on the next poll, so the page shows what
    // actually happened rather than what we assumed.
    if (outcome.conflicts.every((c) => c.scope === "preset")) {
      await saveBinding(fn, key, true);
      return;
    }
    // Cross-slot (another preset in a profile that uses this one) stays as it
    // was: informational, the caller decides. Fan-out is the product.
    pendingKey = key;
    const lines = outcome.conflicts.map((c) =>
      c.scope === "preset"
        ? `${key} also drives this preset's ${c.function}`
        : `${key} is "${c.preset}"'s ${c.function}` +
          (c.profile ? ` (slot ${c.slot} of "${c.profile}")` : ""),
    );
    showConflict(
      prompt(fn).replace("Press the panel key for", "Bind") + ` = ${key}?`,
      lines.join("; ") +
        " — Replace binds it here anyway (other presets are never edited).",
    );
  } else {
    closeModal();
    oops(
      `${name} was not changed: ${outcome.error ?? "the daemon refused the write"}.`,
    );
  }
  void poll(); // zone tags refresh from disk truth
}

/** FEATURE 2's write: the captured key goes to EVERY selected control as N
 *  ordinary `map` calls — which is exactly what a multi-bind is in the preset
 *  file (`A = "P"`, `B = "P"`, `rt = "P"`). A same-preset duplicate needs no
 *  flag at all now — the writer shares the key instead of moving it — so
 *  `force` here only says "yes" to a CROSS-SLOT duplicate, which for a
 *  deliberate map-all is the same intent, and it removes nothing either way.
 *  Sequential on purpose: the writer is one file, and a partial result must be
 *  reportable control by control. */
async function mapAll(fns: string[], key: string): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  // One remembered key per control: the group undo puts each one back where
  // it was, which is not the same as "clear them all" (some were bound).
  const before = new Map<string, string[]>(fns.map((fn) => [fn, previousKeys(fn)]));
  const progress = pushToast(`Binding ${fns.length} controls to ${key}…`);
  const refused: string[] = [];
  for (const fn of fns) {
    const outcome = await bindOnce(slot.preset, fn, key, true);
    if (!outcome.ok) {
      refused.push(`${identityLabel(fn)} (${outcome.error ?? outcome.code ?? "refused"})`);
    }
  }
  // Report from the FILE, not from the requests. A daemon whose `map` verb
  // still MOVES a key rather than sharing it (mapping.rs: "same-preset
  // conflicts are stolen") will accept all N writes and leave only the last
  // one bound — so claiming "one key now drives all three" off the outcomes
  // would be exactly the silent-wipe lie MAPPER-UX commandment 7 forbids.
  // One extra poll, and the sentence is whatever the preset really says.
  await poll();
  const kept = fns.filter((fn) => currentBinding(fn) === key);
  const lost = fns.filter((fn) => currentBinding(fn) !== key);
  if (kept.length > 0) markSaved();

  let line: string;
  let bad = refused.length > 0;
  if (kept.length === fns.length) {
    line =
      `${key} now drives ${kept.length} controls: ` +
      `${kept.map(identityLabel).join(" · ")}.`;
    if (isPaused()) line += " Resume emulation when you're done.";
  } else if (kept.length > 0 && refused.length === 0) {
    // Every write was accepted and they still did not stack: this daemon
    // moves the key instead of sharing it. Name the mechanism, not a shrug.
    bad = true;
    line =
      `${key} ended up on ${kept.map(identityLabel).join(" · ")} only — ` +
      `${lost.map(identityLabel).join(" · ")} did not keep it. This daemon's ` +
      "map verb still MOVES a key between controls instead of letting one key " +
      "drive several; the legend below shows what is really in the preset.";
  } else {
    bad = true;
    line =
      kept.length > 0
        ? `${key} drives ${kept.map(identityLabel).join(" · ")}`
        : `nothing was bound to ${key}`;
  }
  if (refused.length > 0) line += ` — REFUSED: ${refused.join("; ")}`;
  // Undo is offered even on a partial result: putting every control back
  // where it was is exactly the right answer to a write that half-landed.
  const undoable = kept.length > 0 && groupUndoable(before);
  replaceToast(progress, line, {
    kind: bad ? "err" : "ok",
    undo: undoable ? undoGroup(slot.preset, before) : null,
    undone: `Put ${before.size} control${before.size === 1 ? "" : "s"} back the way they were.`,
  });
  clearSelection();
}

/** Clear every selected control in one action (the selection bar's second
 *  button). It just happens — the toast names how many went, and its Undo
 *  puts each one back on the key it actually had (MAPPER-UX commandment 5's
 *  road home, without the toll of a confirm on every correct use). */
async function clearSelectedBindings(): Promise<void> {
  const fns = selectedFns();
  if (fns.length === 0) return;
  if (!learnAllowed()) {
    refuseSelection();
    return;
  }
  const slot = currentSlot();
  if (!slot) return;
  const before = new Map<string, string[]>(fns.map((fn) => [fn, previousKeys(fn)]));
  const done: string[] = [];
  const failed: string[] = [];
  for (const fn of fns) {
    const outcome = await bindOnce(slot.preset, fn, null, false);
    if (outcome.ok) done.push(identityLabel(fn));
    else failed.push(`${identityLabel(fn)} (${outcome.error ?? outcome.code ?? "refused"})`);
  }
  if (done.length > 0) markSaved();
  let line =
    done.length > 0
      ? `Cleared ${done.length} control${done.length === 1 ? "" : "s"}: ${done.join(" · ")}.`
      : "Nothing was cleared.";
  if (failed.length > 0) line += ` FAILED: ${failed.join("; ")}`;
  pushToast(line, {
    kind: failed.length > 0 ? "err" : "ok",
    undo: done.length > 0 && groupUndoable(before) ? undoGroup(slot.preset, before) : null,
    undone: `Put ${before.size} control${before.size === 1 ? "" : "s"} back the way they were.`,
  });
  clearSelection();
  void poll();
}

/** Clear one control. Reached three ways — the modal's button, the legend's
 *  ✕, and Delete/Backspace while the modal is open — all landing here. */
async function clearBinding(fn: string): Promise<void> {
  if (!learnAllowed()) {
    refuse(fn);
    return;
  }
  if (learning()) await cancelLearn();
  await saveBinding(fn, null, false);
}

/** The answer to a click that cannot do anything. Never a no-op: it names the
 *  control, the reason, and the shell command that works anyway. */
function refuse(fn: string): void {
  selectFn(fn);
  const reason = blockedReason() ?? "mapping is unavailable";
  const slot = currentSlot();
  const cli = slot
    ? `ksx map --preset "${slot.preset}" --function ${fn} --key <KEY>`
    : `ksx map --preset <NAME> --function ${fn} --key <KEY>`;
  oops(`Can't learn ${identityLabel(fn)} — ${reason}. From a shell: ${cli}`);
}

/** The same answer for an action that is about a SELECTION, not one control. */
function refuseSelection(): void {
  oops(`Can't map right now — ${blockedReason() ?? "mapping is unavailable"}.`);
}

// ── FIX 0: pause / resume, so the refusal is one click, not a dead end ─────

async function pauseAndMap(): Promise<void> {
  const profile = liveProfile();
  const progress = pushToast("Pausing emulation…");
  const out = await verb("/api/session/stop");
  if (out.ok) {
    markPaused(profile);
    replaceToast(
      progress,
      `Emulation is paused${profile ? ` ("${profile}")` : ""} — map away, then Resume emulation.`,
    );
  } else {
    replaceToast(progress, `Emulation is still running: ${out.error ?? "the daemon refused to stop"}.`, {
      kind: "err",
    });
  }
  void poll();
}

async function resumeEmulation(): Promise<void> {
  const profile = profileToResume();
  const progress = pushToast("Resuming emulation…");
  const out = await verb("/api/session/start", profile ? { profile } : {});
  if (out.ok) {
    clearPaused();
    replaceToast(progress, out.message ?? "Emulation resumed.");
  } else {
    replaceToast(progress, `Emulation did not start: ${out.error ?? "the daemon refused"}.`, {
      kind: "err",
    });
  }
  void poll();
}

// ── Preset-level writes (restore ×3, clear all) ────────────────────────────
// These are the four biggest writes on the page, and until v8 each one hid
// behind a confirm dialog. They now fire on click. What makes that safe is
// not optimism: the daemon copies the preset to
// <preset>.toml.bak-YYYYMMDD-HHMMSS BEFORE it writes (mapping.rs), so the
// NEWEST backup is by construction the state the button just left behind —
// which makes `restore latest-backup` a real single-level undo for all four.
//
// The offer is only made if that backup can be SEEN: after the write we poll
// and read the slot's newest-backup label. No label, no Undo button — a
// button that might restore the wrong state (or nothing) would be worse than
// the confirm dialog it replaced.

type RestoreMode = "defaults" | "session-backup" | "latest-backup";

/** What just happened, as a sentence — no dialog, no question mark. */
function restoreDone(mode: RestoreMode, preset: string): string {
  switch (mode) {
    case "defaults":
      return (
        `"${preset}" now holds the generic keyboard layout (S=A, D=B, A=X, W=Y, ` +
        "Q/E triggers, arrows = left stick, Esc = Start). Every binding it had is gone."
      );
    case "session-backup":
      return `"${preset}" is back to how it was when the daemon started.`;
    case "latest-backup":
      return `"${preset}" is back to its newest timestamped backup.`;
  }
}

/** Finish any whole-preset write: refresh from disk, then offer the backup
 *  that write took as the way back — if the page can actually see one. */
function afterPresetWrite(preset: string): ToastOptions {
  if ((currentSlot()?.backup ?? null) === null) {
    return { kind: "warn" };
  }
  return {
    undo: undoFromBackup(preset),
    undone: `"${preset}" is back to how it was before that.`,
  };
}

async function presetWrite(
  preset: string,
  request: Promise<VerbOutcome>,
  done: string,
  failedLead: string,
): Promise<void> {
  const out = await request;
  if (!out.ok) {
    oops(`${failedLead}: ${out.error ?? "the daemon refused"}.`);
    void poll();
    return;
  }
  markSaved();
  // Poll BEFORE reporting: the backup label the Undo button depends on comes
  // from disk, and so does the legend the user is about to read.
  await poll();
  const opts = afterPresetWrite(preset);
  pushToast(
    opts.undo
      ? done
      : `${done} No backup was found on disk, so this one cannot be undone from here — ` +
        "the restore options in the Preset card are the way back.",
    opts,
  );
}

async function restorePreset(mode: RestoreMode): Promise<void> {
  const preset = currentPreset();
  if (!preset) return;
  await presetWrite(
    preset,
    verb("/api/preset/restore", { preset, mode }),
    restoreDone(mode, preset),
    `"${preset}" was not restored`,
  );
}

async function clearAll(): Promise<void> {
  const preset = currentPreset();
  if (!preset) return;
  await presetWrite(
    preset,
    verb("/api/preset/clear-all", { preset }),
    `Every binding in "${preset}" is cleared — this slot's pad ignores the panel ` +
      "until something is mapped again.",
    `"${preset}" was not cleared`,
  );
}

/** Submit one plain HTML form the way a browser would — form-encoded body,
 *  the submitter's `formaction` honoured — but over fetch, so the page never
 *  navigates. The outcome rides the redirect's `?flash=` (server.rs's
 *  `map_act`), which is the same sentence a no-JS user would have read on the
 *  reloaded page; here it becomes a toast. */
async function submitNoJsForm(
  form: HTMLFormElement,
  submitter: HTMLElement | null,
): Promise<void> {
  const action =
    submitter instanceof HTMLButtonElement && submitter.formAction
      ? submitter.formAction
      : form.action;
  try {
    const body = new URLSearchParams();
    new FormData(form).forEach((value, key) => {
      if (typeof value === "string") body.append(key, value);
    });
    if (submitter instanceof HTMLButtonElement && submitter.name) {
      body.append(submitter.name, submitter.value);
    }
    const res = await fetch(action, { method: "POST", body, redirect: "follow" });
    const flash = new URL(res.url).searchParams.get("flash") ?? "done.";
    pushToast(flash, { kind: flash.startsWith("error") ? "err" : "ok" });
  } catch {
    oops("The request failed — is ksx studio still running?");
  }
  markSaved();
  void poll();
}

// ── v11/v12: the macro editor's controls ───────────────────────────────────
// The grid edits a DRAFT and ONE button writes it: `POST /api/macro/save` →
// `ControlSource::save_macro` → the daemon's `map-macro` verb → the same
// `mapping::save_macro` the `ksx macro` CLI calls. Whole table per call, so
// what the grid holds and what lands in the preset are the same object.
//
// WHY EXPLICIT SAVE and not save-per-edit like the binding side: every macro
// write takes a timestamped backup and hot-swaps the sequence into the running
// session. Autosaving each painted cell would push a half-authored sequence
// into a live game and leave one backup file per click; a grid edit is a
// composition whose unit is the finished sequence. The three STRUCTURAL verbs
// (New / Rename / Delete) are single deliberate actions, so those write
// immediately. Toast + Undo on all four, same as every other write here.
//
// `syncMacroControls` remains what it was: four <select>s, a number input, a
// checkbox and a text input whose VALUES cannot come from an attribute binding
// once the user has touched them (a dirty form control ignores its attribute),
// so the island exposes the draft's truth and this writes it onto the DOM
// after every edit and every poll.

let islandRoot: HTMLElement | null = null;

function syncMacroControls(): void {
  const root = islandRoot;
  if (!root) return;
  const mac = currentMacro();
  const set = (sel: string, value: string): void => {
    const el = root.querySelector<HTMLInputElement | HTMLSelectElement>(sel);
    if (!el || el.value === value) return;
    // NEVER over a control the user's hands are on. This runs after every 2 s
    // poll, and writing into a focused field takes the caret (or the open
    // dropdown) with it — the difference between a page that refreshes and a
    // page that argues with you. Whatever it wanted to say is still true on
    // the next sync, once they have moved on.
    if (el === document.activeElement) return;
    el.value = value;
  };
  // The rate is the author's, not the file's — it survives a macro switch.
  set(".macrate", String(macroTargetRate()));
  if (!mac) {
    set(".macnamein", "");
    return;
  }
  // FIX 2: one box per ROW, so this is a loop rather than a field. Same rule
  // as `set` above — never over the box the caret is in — which is what lets
  // somebody type a duration straight through a 2 s poll.
  for (const box of root.querySelectorAll<HTMLInputElement>(".macrowdur")) {
    const want = macroRowDuration(Number(box.dataset.durrow));
    if (want === "" || box.value === want || box === document.activeElement) continue;
    box.value = want;
  }
  set(".macnamein", mac.name);
  set('.macsel[data-macpol="on_release"]', mac.on_release);
  set('.macsel[data-macpol="retrigger"]', mac.retrigger);
  set('.macsel[data-macpol="interrupt"]', mac.interrupt);
  set('.macsel[data-macpol="repeat"]', macroRepeatValue());
  set(".macturboin", macroTurboBoxValue());
  set(".macturbounit", macroRateUnit());
  const short = root.querySelector<HTMLInputElement>(".macshortin");
  if (short) short.checked = macroStepAllowShort();
}

/** The text in the "new macro name" box, trimmed. */
function newMacroName(): string {
  return islandRoot?.querySelector<HTMLInputElement>(".macnewin")?.value.trim() ?? "";
}

/** The text in the name box beside the grid, trimmed — Rename's argument. */
function typedMacroName(): string {
  return islandRoot?.querySelector<HTMLInputElement>(".macnamein")?.value.trim() ?? "";
}

function clearNewMacroName(): void {
  const el = islandRoot?.querySelector<HTMLInputElement>(".macnewin");
  if (el) el.value = "";
}

// ── The one macro writer ───────────────────────────────────────────────────

/** One whole-table write (or delete). Never throws — transport failure comes
 *  back in the same shape, so no caller can forget it. */
async function macroWrite(preset: string, mac: MacroView, remove = false): Promise<MacroOutcome> {
  try {
    return await fetchJSON<MacroOutcome>("/api/macro/save", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        preset,
        name: mac.name,
        // A delete carries no body: an EMPTY step list is a refusal on the
        // daemon side precisely so an editor that lost its grid cannot delete
        // a macro by omission (control.rs `MacroWrite::delete`).
        steps: remove ? [] : mac.steps,
        on_release: mac.on_release,
        retrigger: mac.retrigger,
        interrupt: mac.interrupt,
        repeat: mac.repeat,
        // Two spellings of one number, so exactly ONE is sent: the daemon
        // refuses a table that gives both, and a stale companion field would
        // turn an editor slip into that refusal.
        ...(mac.turbo_hz !== null
          ? { turbo_hz: mac.turbo_hz }
          : mac.gap_ms !== null
            ? { gap_ms: mac.gap_ms }
            : {}),
        delete: remove,
        // The server forces this on anyway; sent so the request says what it
        // means. A macro body is a binding change — the pads stay plugged.
        reload: true,
      }),
    });
  } catch {
    return {
      ok: false,
      message: null,
      error: "the macro save request failed — is ksx studio still running?",
      code: null,
      problems: [],
      warnings: [],
      deleted: false,
      backup: null,
      reloaded: false,
    };
  }
}

/** The TOGGLE request: `enabled` and no body. See `macroToggleEnabled`. */
async function macroSetEnabled(
  preset: string,
  name: string,
  enabled: boolean,
): Promise<MacroOutcome> {
  try {
    return await fetchJSON<MacroOutcome>("/api/macro/save", {
      method: "POST",
      headers: { "content-type": "application/json" },
      // NO `steps`: that is what makes this a toggle rather than a write of
      // whatever the grid happens to be holding.
      body: JSON.stringify({ preset, name, enabled, reload: true }),
    });
  } catch {
    return {
      ok: false,
      message: null,
      error: "the macro switch request failed — is ksx studio still running?",
      code: null,
      problems: [],
      warnings: [],
      deleted: false,
      backup: null,
      reloaded: false,
    };
  }
}

/** A refusal, in one sentence: the daemon's own words plus every problem row
 *  it listed (validation names them one per fault — a step holding `warp`, a
 *  duration in two units — and swallowing them would leave the user guessing
 *  which step is wrong). */
function macroRefusal(out: MacroOutcome): string {
  const lead = out.error ?? out.code ?? "the daemon refused the write";
  return out.problems.length === 0 ? lead : `${lead} — ${out.problems.join("; ")}`;
}

/** The advisories a SUCCESSFUL write still has to say out loud (a step below
 *  the sampling floor was raised, or runs as written and may be missed). */
function macroNotes(out: MacroOutcome): string {
  return out.warnings.length === 0 ? "" : ` Note: ${out.warnings.join("; ")}.`;
}

/** Put a macro back exactly as it was — the undo of a save, a delete, or half
 *  of a rename. `null` = it did not exist before, so the undo is a delete. */
function undoMacroTo(
  preset: string,
  before: MacroView | null,
  name: string,
): () => Promise<string | null> {
  return async () => {
    if (before === null) {
      const out = await macroWrite(preset, { ...newMacroBody(name) }, true);
      if (!out.ok) return macroRefusal(out);
    } else {
      const out = await macroWrite(preset, before);
      if (!out.ok) return macroRefusal(out);
      // The table is back; so are its trigger rows, which live in [bindings]
      // and are written by the `map` verb, not by this one.
      if (before.triggers.length > 0) {
        const back = await bindKeys(preset, `macro.${before.name}`, before.triggers, true);
        if (!back.ok) {
          return `the macro is back, but its trigger key(s) are not: ${back.error ?? "refused"}`;
        }
      }
    }
    markSaved();
    await poll();
    seedMacro(before === null ? null : before.name);
    syncMacroControls();
    return null;
  };
}

/** What a macro verb needs before it can do anything, or the sentence saying
 *  why it cannot. Never a silent return: a button that answers nothing is the
 *  one thing this page does not ship. */
function macroTarget(): { mac: MacroView; preset: string } | null {
  const mac = currentMacro();
  const preset = currentPreset();
  if (!preset) {
    oops("No slot is selected, so there is no preset to write a macro into.");
    return null;
  }
  if (!mac) {
    oops(
      "No macro is loaded. Pick one from the tabs above, or type a name and press ＋ New macro.",
    );
    return null;
  }
  return { mac, preset };
}

/** SAVE: the whole draft table into the preset.
 *
 *  FIX 2 — a draft holding a step below the sampling floor is not saved on the
 *  first click. The card ASKS, inline, with the count in it, and the second
 *  click writes it exactly as authored. Never a refusal: a short step is legal
 *  and `allow_short` exists so one can be written on purpose. What it stops is
 *  the SILENT save — which is how an evening went into a hadouken whose steps
 *  were one frame long, with the explanation sitting under the grid being read
 *  as decoration. */
async function macroSave(confirmed = false): Promise<void> {
  const target = macroTarget();
  if (!target) return;
  const { mac, preset } = target;
  if (!macroIsDirty()) {
    pushToast(`"${mac.name}" already matches the preset file — nothing to save.`, { kind: "warn" });
    return;
  }
  // The question, once. `macroAskAboutShortSteps` answers false when there is
  // nothing below the floor, which is the ordinary save and costs no click.
  if (!confirmed && macroAskAboutShortSteps()) return;
  macroClearShortStepQuestion();
  // Read BEFORE the write: this is the entire undo.
  const before = macroOnDiskCopy(mac.name);
  const out = await macroWrite(preset, mac);
  if (!out.ok) {
    oops(`"${mac.name}" was NOT saved: ${macroRefusal(out)}. The preset file is untouched.`);
    return;
  }
  markMacroSaved(mac.name);
  markSaved();
  await poll();
  seedMacro(mac.name);
  syncMacroControls();
  let line =
    `"${mac.name}" saved into "${preset}" — ${mac.steps.length} step` +
    `${mac.steps.length === 1 ? "" : "s"}.${macroNotes(out)}`;
  // Confirmed is not forgotten: the toast repeats what was agreed to, so the
  // fact survives the bar coming down.
  const short = macroShortStepQuestion();
  if (short !== "") line += ` Saved as authored: ${short.replace(/\. Save anyway\?$/, ".")}`;
  if (out.reloaded) line += " The running session is already playing this version.";
  pushToast(line, {
    kind: out.warnings.length > 0 || short !== "" ? "warn" : "ok",
    undo: undoMacroTo(preset, before, mac.name),
    undone:
      before === null
        ? `"${mac.name}" is gone again.`
        : `"${mac.name}" is back to the version that was in the file.`,
  });
}

/** NEW: create the macro in the preset, right now, with one starter step. */
async function macroNew(): Promise<void> {
  const preset = currentPreset();
  if (!preset) {
    oops("No slot is selected, so there is no preset to create a macro in.");
    return;
  }
  const name = newMacroName();
  const problem = macroNameProblem(name);
  if (problem !== null) {
    oops(problem);
    return;
  }
  // Creating switches the editor, so an unsaved grid is about to be dropped.
  // Say it before it happens, naming what it was.
  const leaving = currentMacro();
  if (macroIsDirty() && leaving) {
    pushToast(
      `Unsaved changes to "${leaving.name}" were discarded — Save macro writes them, ` +
        "creating another macro does not. (The preset file is unchanged.)",
      { kind: "warn" },
    );
  }
  const out = await macroWrite(preset, newMacroBody(name));
  if (!out.ok) {
    oops(`"${name}" was NOT created: ${macroRefusal(out)}`);
    return;
  }
  clearNewMacroName();
  markSaved();
  await poll();
  seedMacro(name);
  syncMacroControls();
  pushToast(
    `"${name}" now exists in "${preset}" — one empty 50 ms step. Paint the grid, press ` +
      "Save macro, then bind a trigger key at the bottom of this card." +
      macroNotes(out),
    {
      undo: undoMacroTo(preset, null, name),
      undone: `"${name}" is gone again.`,
    },
  );
}

/** RENAME: save under the new name, delete the old table, then move the
 *  trigger keys across. Three calls of two verbs that already exist — there is
 *  no rename verb, and inventing one here would be a second macro writer.
 *  Reported as ONE action, and its Undo is the exact inverse. */
async function macroRenameTo(
  preset: string,
  from: MacroView,
  to: string,
): Promise<string | null> {
  const moved: MacroView = { ...from, name: to, steps: from.steps, triggers: [...from.triggers] };
  const wrote = await macroWrite(preset, moved);
  if (!wrote.ok) return `${macroRefusal(wrote)} — nothing was renamed`;
  const removed = await macroWrite(preset, from, true);
  if (!removed.ok) {
    return (
      `"${to}" was written, but the old "${from.name}" table could NOT be removed ` +
      `(${macroRefusal(removed)}) — the preset now holds both. Delete one of them.`
    );
  }
  // Deleting the old table took its `macro.<old>` trigger rows with it (they
  // would not load without it), so the keys have to be pointed at the new
  // name. That is an ordinary binding write, through the `map` verb.
  if (from.triggers.length > 0) {
    const bound = await bindKeys(preset, `macro.${to}`, from.triggers, true);
    if (!bound.ok) {
      return (
        `"${from.name}" is now "${to}", but its trigger key(s) ${keyList(from.triggers)} ` +
        `could not be moved across (${bound.error ?? "refused"}) — set the trigger again below`
      );
    }
  }
  return null;
}

async function macroRename(): Promise<void> {
  const target = macroTarget();
  if (!target) return;
  const { mac, preset } = target;
  const to = typedMacroName();
  if (to === mac.name) {
    pushToast("That is already this macro's name — nothing to rename.", { kind: "warn" });
    return;
  }
  // A rename that only changes CASE would destroy the macro: the writer
  // matches names case-insensitively and keeps the file's own spelling
  // (mapping.rs `save_macro`), so "Hadouken" would land back in the
  // "hadouken" table — and the delete that follows would then take it away
  // entirely. Refused in those words rather than performed as data loss.
  if (to.toLowerCase() === mac.name.toLowerCase()) {
    pushToast(
      `Macro names are matched without case, so "${to}" and "${mac.name}" are the same ` +
        "table and the file keeps the spelling it already has. Nothing was changed — pick a " +
        "different name.",
      { kind: "warn" },
    );
    return;
  }
  const problem = macroNameProblem(to, mac.name);
  if (problem !== null) {
    oops(`Not renamed. ${problem}`);
    return;
  }
  const from = { ...mac, triggers: [...mac.triggers] };
  const failure = await macroRenameTo(preset, from, to);
  await poll();
  if (failure !== null) {
    oops(`Rename problem: ${failure}.`);
    seedMacro(null);
    syncMacroControls();
    return;
  }
  markMacroSaved(to);
  markSaved();
  seedMacro(to);
  syncMacroControls();
  pushToast(
    `"${from.name}" is now "${to}" in "${preset}"` +
      (from.triggers.length > 0
        ? `, and ${keyList(from.triggers)} still starts it.`
        : ". It has no trigger key yet — set one below."),
    {
      undo: async () => {
        const back = await macroRenameTo(preset, { ...from, name: to }, from.name);
        markSaved();
        await poll();
        seedMacro(from.name);
        syncMacroControls();
        return back;
      },
      undone: `It is called "${from.name}" again.`,
    },
  );
}

/** SWITCH ON/OFF: the one write on this card that touches nothing but a flag.
 *
 *  Sent WITHOUT `steps`, which is the daemon's toggle spelling (pipe.rs
 *  `map-macro`): the table on disk keeps every step and every policy, and only
 *  `enabled` moves. That matters more than it looks — the whole reason to
 *  disable rather than delete is that what comes back is exactly what went
 *  away, and a toggle that re-sent the browser's draft could not promise it
 *  (the grid may be dirty, and it may be dirty in a way that would not save).
 *
 *  So this deliberately ignores the draft and reads the ON-DISK state. */
async function macroToggleEnabled(): Promise<void> {
  const target = macroTarget();
  if (!target) return;
  const { mac, preset } = target;
  if (!macroIsOnDisk()) {
    pushToast(
      `"${mac.name}" is not in the preset file yet, so there is nothing to switch off. ` +
        "Press Save macro first.",
      { kind: "warn" },
    );
    return;
  }
  const onDisk = macroOnDiskCopy(mac.name);
  const wasDisabled = onDisk?.disabled === true;
  const out = await macroSetEnabled(preset, mac.name, wasDisabled);
  if (!out.ok) {
    oops(
      `"${mac.name}" was NOT switched ${wasDisabled ? "on" : "off"}: ${macroRefusal(out)}. ` +
        "The preset file is untouched.",
    );
    return;
  }
  markSaved();
  await poll();
  syncMacroControls();
  const line = wasDisabled
    ? `"${mac.name}" is ON again in "${preset}" — its trigger starts it as before.`
    : `"${mac.name}" is OFF in "${preset}" — it keeps its steps and its trigger row, and ` +
      "never runs. Switch it back on any time; nothing was lost." +
      (mac.triggers.length > 0 ? ` ${keyList(mac.triggers)} now starts nothing.` : "");
  pushToast(line + macroNotes(out), {
    kind: "ok",
    // The undo is the opposite flag — one field back, exactly like the write.
    undo: async () => {
      await macroSetEnabled(preset, mac.name, !wasDisabled);
      await poll();
      syncMacroControls();
    },
    undone: wasDisabled
      ? `"${mac.name}" is switched off again.`
      : `"${mac.name}" runs again.`,
  });
}

/** DELETE: remove the table (and the trigger rows that would dangle). */
async function macroDelete(): Promise<void> {
  const target = macroTarget();
  if (!target) return;
  const { mac, preset } = target;
  if (!macroIsOnDisk()) {
    pushToast(`"${mac.name}" is not in the preset file, so there is nothing to delete.`, {
      kind: "warn",
    });
    return;
  }
  const before = macroOnDiskCopy(mac.name) ?? { ...mac, triggers: macroDraftTriggers() };
  const out = await macroWrite(preset, mac, true);
  if (!out.ok) {
    oops(`"${mac.name}" was NOT deleted: ${macroRefusal(out)}`);
    return;
  }
  markSaved();
  await poll();
  seedMacro(null);
  syncMacroControls();
  pushToast(
    `"${mac.name}" is gone from "${preset}"` +
      (before.triggers.length > 0
        ? ` — ${keyList(before.triggers)} no longer starts anything.`
        : "."),
    {
      undo: undoMacroTo(preset, before, mac.name),
      undone: `"${mac.name}" is back, exactly as it was.`,
    },
  );
}

/** Any editable control in the island holds the caret. Hover highlighting
 *  re-derives the zone and legend lists, which REBUILDS their DOM — and a
 *  rebuild under a focused control takes the focus with it. A highlight is
 *  cosmetic; a half-finished edit is not, so the highlight yields. */
function islandIsBeingEdited(): boolean {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement) || islandRoot === null) return false;
  if (!islandRoot.contains(active)) return false;
  return (
    active instanceof HTMLInputElement ||
    active instanceof HTMLSelectElement ||
    active instanceof HTMLTextAreaElement
  );
}

/** One `verb|index` payload from a row button. */
function macroAct(payload: string): void {
  const at = payload.indexOf("|");
  if (at <= 0) return;
  macroStepVerb(payload.slice(0, at), Number(payload.slice(at + 1)));
  syncMacroControls();
}

/** One `stepIndex|function` payload from a grid cell.
 *
 *  A DIAGONAL pick reports: it is the one click on this grid whose effect is not
 *  literally the cell you hit (ksx stores the pair), so it says so — with the
 *  two names it wrote, and with Undo, the same parity every other write here
 *  has. A plain cardinal or button toggle says nothing: the cell itself is the
 *  whole report. */
function macroCell(payload: string): void {
  const at = payload.indexOf("|");
  if (at < 0) return;
  const outcome = macroToggleCell(Number(payload.slice(0, at)), payload.slice(at + 1));
  syncMacroControls();
  if (outcome === null) return;
  pushToast(`${outcome.said} (Nothing is written until you press Save macro.)`, {
    undo: async () => {
      const refused = outcome.undo();
      syncMacroControls();
      return refused;
    },
    undone: "Put that step's holds back.",
  });
}

/** FIX 1c: one of the ready-made motions, appended to the draft. The toast is
 *  the teaching — it names which step holds two controls and why that is what
 *  a diagonal is, because the point of the helper is the concept, not the
 *  typing it saves. */
function macroMotion(name: string): void {
  if (!currentMacro()) {
    oops(
      "No macro is loaded, so there is nothing to add a motion to. Pick one from the tabs " +
        "above, or type a name and press ＋ New macro.",
    );
    return;
  }
  const said = macroInsertMotion(name);
  if (said === null) return;
  syncMacroControls();
  pushToast(`${said} (Nothing is written until you do — "Revert to file" undoes this.)`);
}

/** Switch the editor to another of the preset's macros. Unsaved grid edits are
 *  DISCARDED — and said so, naming what they were, because the one thing this
 *  page never does is lose work quietly. */
function macroSwitch(name: string | null): void {
  const leaving = currentMacro();
  if (macroIsDirty() && leaving) {
    pushToast(
      `Unsaved changes to "${leaving.name}" were discarded — Save macro writes them, ` +
        "switching macros does not. (The preset file is unchanged.)",
      { kind: "warn" },
    );
  }
  seedMacro(name);
  syncMacroControls();
}

async function macroCopy(): Promise<void> {
  const text = macroTomlText();
  if (text === "") return;
  try {
    await navigator.clipboard.writeText(text);
    pushToast(
      "The [macros] block is on the clipboard — for sharing or hand-editing. " +
        "(Save macro already writes it into the preset for you.)",
    );
  } catch {
    oops("Could not reach the clipboard — select the block and copy it by hand.");
  }
}

// ── Wiring: delegated events on the island root ────────────────────────────

function wire(root: HTMLElement): void {
  islandRoot = root;
  // Multi-select is a JS enhancement: the "Select multiple" toggle stays
  // hidden until this class exists, so a no-JS page never shows a control that
  // cannot do anything (the whole page's standing rule — FIX 1).
  root.classList.add("js");

  root.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement | null;
    if (!target) return;

    // ── The toast stack, checked first: it floats over the page, so its
    // buttons must never be read as something underneath them. ───────────
    const undoId = target.closest<HTMLElement>("[data-undo]")?.dataset.undo;
    if (undoId) {
      ev.preventDefault();
      void runUndo(undoId);
      return;
    }
    const dismissId = target.closest<HTMLElement>("[data-dismiss]")?.dataset.dismiss;
    if (dismissId) {
      ev.preventDefault();
      dismissToast(dismissId);
      return;
    }

    // The per-key ✕ (v10), checked before BOTH the row's clear and its
    // data-fn: all three live inside the same row button, and this is the
    // most specific of them. Payload is `function|KEY` — the key can contain
    // no `|` (Key::name() spellings are alphanumeric), so the FIRST separator
    // splits it.
    const rmkey = target.closest<HTMLElement>("[data-rmkey]")?.dataset.rmkey;
    if (rmkey) {
      ev.preventDefault();
      const at = rmkey.indexOf("|");
      if (at > 0) {
        const fn = rmkey.slice(0, at);
        const key = rmkey.slice(at + 1);
        if (learnAllowed()) void removeKey(fn, key);
        else refuse(fn);
      }
      return;
    }

    // ── v11: the macro grid. Checked before `data-fn`, because the macro
    // card's Trigger button carries one and nothing else in here does. ────
    const cell = target.closest<HTMLElement>("[data-cell]")?.dataset.cell;
    if (cell) {
      ev.preventDefault();
      macroCell(cell);
      return;
    }
    const macact = target.closest<HTMLElement>("[data-macact]")?.dataset.macact;
    if (macact) {
      ev.preventDefault();
      macroAct(macact);
      return;
    }
    const motion = target.closest<HTMLElement>("[data-macmotion]")?.dataset.macmotion;
    if (motion) {
      ev.preventDefault();
      macroMotion(motion);
      return;
    }
    const macro = target.closest<HTMLElement>("[data-macro]")?.dataset.macro;
    if (macro) {
      // The tab is an <a href="/map?slot=N&macro=NAME"> so it works with JS
      // off; with JS on we switch in place and keep the URL honest.
      ev.preventDefault();
      macroSwitch(macro);
      const slot = currentSlot();
      window.history.replaceState(
        null,
        "",
        `/map?slot=${slot ? slot.number : 1}&macro=${encodeURIComponent(macro)}`,
      );
      return;
    }

    // The legend's ✕ accelerator, checked BEFORE the row's own data-fn: the
    // span lives inside the row button, so both would match otherwise.
    const clear = target.closest<HTMLElement>("[data-clear]")?.dataset.clear;
    if (clear) {
      ev.preventDefault();
      void clearBinding(clear);
      return;
    }

    const act = target.closest<HTMLElement>("[data-act]")?.dataset.act;
    if (act === "replace") {
      const fn = selectedFnName();
      if (fn && pendingKey) void saveBinding(fn, pendingKey, true);
      return;
    }
    if (act === "cancel") {
      void cancelLearn();
      return;
    }
    // v10: pick what the press that is being waited for will DO. Nothing is
    // written here — the capture is still running, this only arms it, and the
    // "currently …" line re-states the choice so the mode is never hidden.
    if (act === "mode-add" || act === "mode-replace") {
      learnMode = act === "mode-add" ? "add" : "replace";
      const fn = selectedFnName();
      showLearnMode(learnMode === "add", fn ? currentBinding(fn) : null);
      return;
    }
    if (act === "clear-one") {
      const fn = selectedFnName();
      if (fn) void clearBinding(fn);
      return;
    }
    // v13: AUTO-FIRE, in the modal's own Add/Replace/Clear vocabulary. It
    // writes through the SAME `bind_keys` seam with the control's CURRENT key
    // list — turbo belongs to the CONTROL, so setting it is that control's
    // write with one more field, not a second writer.
    if (act === "turbo-set" || act === "turbo-clear") {
      const fn = selectedFnName();
      if (fn) void setTurbo(fn, act === "turbo-clear" ? 0 : modalTurboInput());
      return;
    }
    if (act === "pause-map") {
      void pauseAndMap();
      return;
    }
    if (act === "resume") {
      void resumeEmulation();
      return;
    }
    if (act === "clear-all") {
      void clearAll();
      return;
    }
    // ── FEATURE 2: multi-select ──────────────────────────────────────────
    if (act === "multi-toggle") {
      if (!learnAllowed()) {
        refuseSelection();
        return;
      }
      setMultiMode(!isMultiMode());
      return;
    }
    if (act === "map-selected") {
      const fns = selectedFns();
      if (fns.length === 0) return;
      if (!learnAllowed()) {
        refuseSelection();
        return;
      }
      void startLearn(fns);
      return;
    }
    if (act === "clear-selected") {
      void clearSelectedBindings();
      return;
    }
    if (act === "cancel-select") {
      // One exit for both entry points: drop the selection AND leave the
      // touch mode, so "Cancel" never leaves taps still selecting.
      setMultiMode(false);
      return;
    }
    // ── v11/v12: macro-card verbs. Four of them WRITE, through the one
    // `save_macro` seam (= the daemon's `map-macro`); the rest move the
    // draft. ─────────────────────────────────────────────────────────────
    if (act === "macro-save") {
      void macroSave();
      return;
    }
    // FIX 2: the answers to Save's short-step question. "Save anyway" is the
    // same write with the asking skipped; "Not yet" just takes the bar down
    // and leaves the preset alone.
    if (act === "macro-save-anyway") {
      void macroSave(true);
      return;
    }
    if (act === "macro-save-cancel") {
      macroClearShortStepQuestion();
      pushToast(
        "Nothing was written — the preset file is untouched. The amber rows are the short " +
          "steps: pick one's ⏱ and give it 33 ms (2 frames) or more, and the flag goes away. " +
          "If you meant it, Save anyway writes it exactly as authored.",
        { kind: "warn" },
      );
      return;
    }
    if (act === "macro-new") {
      void macroNew();
      return;
    }
    if (act === "macro-rename") {
      void macroRename();
      return;
    }
    if (act === "macro-delete") {
      void macroDelete();
      return;
    }
    if (act === "macro-enable") {
      void macroToggleEnabled();
      return;
    }
    if (act === "macro-addstep") {
      macroAct("add|0");
      return;
    }
    if (act === "macro-revert") {
      // Back to the TABLE it was seeded from, not to the draft's current name
      // — a rename must not become a one-way door.
      seedMacro(macroSeededFrom());
      syncMacroControls();
      return;
    }
    if (act === "macro-copy") {
      void macroCopy();
      return;
    }
    if (act === "restore-defaults" || act === "restore-backup" || act === "restore-latest") {
      const mode: RestoreMode =
        act === "restore-defaults"
          ? "defaults"
          : act === "restore-backup"
            ? "session-backup"
            : "latest-backup";
      void restorePreset(mode);
      return;
    }

    // Click-away on the modal backdrop cancels.
    if ((target as HTMLElement).dataset?.cancel) {
      void cancelLearn();
      return;
    }

    const tab = target.closest<HTMLElement>("[data-slot]");
    if (tab?.dataset.slot) {
      // v9: the tab is an <a href="/map?slot=N"> so it works with JS off.
      // With JS on we switch in place — no navigation, no lost scroll — and
      // keep the URL honest so a reload lands on the same slot.
      ev.preventDefault();
      void cancelLearn();
      selectSlot(Number(tab.dataset.slot));
      window.history.replaceState(null, "", `/map?slot=${tab.dataset.slot}`);
      return;
    }

    // v12: the trigger block while the preset holds no macro. Its learn button
    // carries an EMPTY data-fn (there is nothing to point a key at), which the
    // branch below would read as "not a zone" and do nothing at all. Say why
    // instead — a dead click is the silent no-op this page bans.
    const inertTrigger = target.closest<HTMLElement>(".mactrigger.off");
    if (inertTrigger && target.closest("button") !== null) {
      ev.preventDefault();
      oops(
        "There is no macro for a key to start yet. Create one with ＋ New macro, then set " +
          "its trigger here.",
      );
      return;
    }

    const zone = target.closest<HTMLElement>("[data-fn]");
    if (zone?.dataset.fn) {
      const fn = zone.dataset.fn;
      // Victor's file-explorer analogy: Ctrl/Shift/⌘-click ADDS to a
      // selection — and on touch, where no modifier exists, the header's
      // "Select multiple" toggle makes every plain tap do the same. A macro
      // TRIGGER is never part of that selection: "map all to one key" is
      // about pad controls on the art, and a sequence's start key is not one.
      const additive =
        !fn.startsWith("macro.") &&
        (ev.ctrlKey || ev.metaKey || ev.shiftKey || isMultiMode());
      if (additive) {
        ev.preventDefault();
        if (!learnAllowed()) {
          refuse(fn); // selecting what cannot be mapped would be a dead end
          return;
        }
        toggleSelected(fn);
        return;
      }
      // A plain click is the single-control flow, and (like an explorer) it
      // drops any selection rather than silently acting on a stale one.
      if (selectionCount() > 0) clearSelection();
      if (learnAllowed()) {
        void startLearn([fn]);
      } else {
        // FIX 1: never a silent no-op. Say which control, why it cannot be
        // learned, and the shell one-liner that works anyway.
        refuse(fn);
      }
    }
  });

  // ── v9: the no-JS forms, fetch-enhanced ────────────────────────────────
  // Every mapper action is a real <form method="post"> now (see MapIsland's
  // `.nojs` surfaces and the preset card), so the page works with JavaScript
  // switched off: POST → 303 → /map?slot=N&flash=… → the server-rendered
  // flash line. With JavaScript on, nothing may navigate:
  //
  //   • a form whose button carries `data-act` was ALREADY handled by the
  //     click delegation above (richer path: optimistic write, toast, Undo),
  //     so here we only cancel the navigation — reporting it twice would be
  //     worse than not reporting it at all;
  //   • the rest (the row forms and the bind-by-name panel — hidden by CSS
  //     under `.js`, but reachable by a stray Enter) are submitted by fetch
  //     and their redirect's ?flash= becomes a toast.
  root.addEventListener("submit", (ev) => {
    const form = ev.target as HTMLFormElement | null;
    if (!form) return;
    ev.preventDefault();
    if (form.querySelector("[data-act]")) return;
    void submitNoJsForm(form, (ev as SubmitEvent).submitter);
  });

  // ── v11: the macro editor's form controls ──────────────────────────────
  // Every one of them moves the DRAFT and nothing else; the TOML block below
  // the grid is the output, and it re-renders on the same tick.
  const macroInput = (ev: Event): void => {
    const el = ev.target as HTMLElement | null;
    if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLSelectElement)) return;
    // The two TEXT boxes commit on `change` (blur / Enter / spinner), never on
    // `input`: a repaint mid-keystroke rewrites the very field being typed
    // into, and a caret that jumps to the end on every character is not an
    // editor. The selects and the checkbox have no caret, so they are live.
    const committed = ev.type === "change";
    if (el.classList.contains("macrowdur")) {
      // FIX 2: the row's OWN duration box — no step has to be selected first,
      // and the row index rides on the element rather than in a mode. The
      // unit comes from the DRAFT (that row's authored unit), never from
      // reading a control back, which is where the old "it just resets" came
      // from. Committing repaints the row (the words beside the box, the amber
      // class), which rebuilds it — so the caret is put back where it was, and
      // Enter-to-commit stays a thing you can do twice in a row.
      if (!committed) return;
      const row = Number(el.dataset.durrow);
      macroSetDurationAt(row, Number(el.value));
      syncMacroControls();
      const again = islandRoot?.querySelector<HTMLInputElement>(
        `.macrowdur[data-durrow="${row}"]`,
      );
      if (again && again !== el) again.focus();
      return;
    }
    if (el.classList.contains("macturboin")) {
      // A number box, so it commits on `change` for the same caret reason the
      // duration box does.
      if (committed) macroSetTurboRate(el.value, macroRateUnit());
      return;
    }
    if (el.classList.contains("macturbounit")) {
      // MOVES the value between the two spellings rather than adding a second
      // field: a table giving both `turbo_hz` and `gap_ms` is refused.
      const box = islandRoot?.querySelector<HTMLInputElement>(".macturboin");
      macroSetTurboRate(box?.value ?? "", el.value);
      syncMacroControls();
      return;
    }
    if (el instanceof HTMLInputElement && el.classList.contains("macshortin")) {
      macroSetAllowShort(el.checked);
      return;
    }
    if (el.classList.contains("macrate")) {
      // Display-only (MapIsland's `setMacroTargetRate` says why): it converts
      // frames ↔ ms for the author and changes nothing in the file.
      setMacroTargetRate(Number(el.value));
      return;
    }
    if (el.classList.contains("macnamein")) {
      // Typing a name changes NOTHING on its own — the Rename button is the
      // write. The old build renamed the draft here, which is why a rename
      // "just reset back": the next poll re-seeded from a file that had never
      // heard of the new name.
      return;
    }
    if (el.classList.contains("macnewin")) {
      return; // ＋ New macro is the write; the box is just a name
    }
    const field = el.dataset.macpol;
    if (field) macroSetPolicy(field, el.value);
  };
  root.addEventListener("input", macroInput);
  root.addEventListener("change", macroInput);

  // Right-click on a zone or legend row is a DESKTOP BONUS path to clear —
  // never the only one (this page is meant for a phone at the cabinet, where
  // there is no right-click at all).
  root.addEventListener("contextmenu", (ev) => {
    const fn = (ev.target as HTMLElement | null)?.closest<HTMLElement>("[data-fn]")?.dataset.fn;
    if (!fn) return;
    ev.preventDefault();
    void clearBinding(fn);
  });

  // The shared hover signal: any element carrying data-fn (a zone on the art
  // OR a legend row) hot-highlights BOTH renderings of that function; leaving
  // it (or the island) clears. focusin keeps keyboard users in sync.
  const hotFrom = (ev: Event): void => {
    // v12.1: never while something in the island is being typed into. The
    // highlight rebuilds the zone and legend lists, and a pointer that merely
    // crosses the card must not be able to reach into a field somebody is
    // still filling in.
    if (islandIsBeingEdited()) return;
    const el = (ev.target as HTMLElement | null)?.closest<HTMLElement>("[data-fn]");
    setHot(el?.dataset.fn ?? null);
  };
  root.addEventListener("mouseover", hotFrom);
  root.addEventListener("focusin", hotFrom);
  root.addEventListener("mouseleave", () => {
    if (!islandIsBeingEdited()) setHot(null);
  });

  // The macro editor's own focus, tracked so the 2 s poll can tell "a draft
  // nobody is touching" (re-seed it from the file) from "an edit in progress"
  // (leave it alone). focusout fires BEFORE the new element takes focus, so
  // the answer is computed from relatedTarget, not from activeElement.
  const macroFocusChanged = (ev: Event): void => {
    const to =
      ev.type === "focusout" ? (ev as FocusEvent).relatedTarget : (ev.target as EventTarget | null);
    setMacroEditorFocused(to instanceof HTMLElement && to.closest(".macedit") !== null);
  };
  root.addEventListener("focusin", macroFocusChanged);
  root.addEventListener("focusout", macroFocusChanged);

  // A toast must not vanish under the hand reaching for its Undo button, so
  // pointer or keyboard focus anywhere in the stack freezes every timer in
  // it; leaving lets them run again. `mouseout`/`focusout` fire on the way
  // BETWEEN toasts too, hence the relatedTarget check.
  const inToasts = (node: EventTarget | null): boolean =>
    node instanceof HTMLElement && node.closest(".toasts") !== null;
  root.addEventListener("mouseover", (ev) => {
    if (inToasts(ev.target)) holdToasts();
  });
  root.addEventListener("mouseout", (ev) => {
    if (inToasts(ev.target) && !inToasts((ev as MouseEvent).relatedTarget)) releaseToasts();
  });
  root.addEventListener("focusin", (ev) => {
    if (inToasts(ev.target)) holdToasts();
  });
  root.addEventListener("focusout", (ev) => {
    if (inToasts(ev.target) && !inToasts((ev as FocusEvent).relatedTarget)) releaseToasts();
  });

  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      // One key, one road out, most-specific first: cancel the capture, close
      // the modal, drop the selection, leave select mode.
      if (learning()) {
        void cancelLearn();
        return;
      }
      if (modalIsOpen()) {
        closeModal();
        return;
      }
      if (selectionCount() > 0) {
        clearSelection();
        return;
      }
      if (isMultiMode()) setMultiMode(false);
      return;
    }
    // Ctrl-Z: the desktop reflex, pointed at the newest toast that still has
    // an Undo button. Never while a learn modal owns the keyboard (the focus
    // guard is swallowing keys for the daemon there), and never while the
    // user is typing into something.
    if ((ev.ctrlKey || ev.metaKey) && !ev.shiftKey && !ev.altKey && ev.key.toLowerCase() === "z") {
      if (modalIsOpen() || learning()) return;
      const active = document.activeElement;
      if (
        active instanceof HTMLElement &&
        (active.isContentEditable || /^(input|textarea|select)$/i.test(active.tagName))
      ) {
        return;
      }
      const id = newestUndoable();
      if (!id) return;
      ev.preventDefault();
      void runUndo(id);
      return;
    }
    // MAME's UI Clear, keyboard edition — ONLY while the modal is open, so it
    // can never fire at a control the user is merely hovering.
    if ((ev.key === "Delete" || ev.key === "Backspace") && modalIsOpen()) {
      const fn = selectedFnName();
      if (fn) {
        ev.preventDefault();
        void clearBinding(fn);
      }
    }
  });
}

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`).
 *
 *  Deliberately NOT the island's `props` argument — see status.ts for the
 *  full note. Short version: compiler 0.3.1 populates island `slot_ids`, so
 *  forma-ir emits `data-forma-props` and core prefers it, which makes `props`
 *  the rendered SLOT values. This page edits a MODEL (bindings, macro drafts,
 *  undo, conflicts) that no slot carries, so it reads the payload itself.
 *  Dogfood ledger #8 (closed) / #19 (the gap it left). */
function embeddedPayload<T>(): T | null {
  const el = document.getElementById("__ksx-payload");
  if (!el?.textContent) return null;
  try {
    return JSON.parse(el.textContent) as T;
  } catch {
    return null;
  }
}

activateIslands({
  // Ledger #5 order: signals seeded from the payload BEFORE adoption.
  MapIsland: (el) => {
    const seed = embeddedPayload<MapPayload>();
    if (seed) {
      // Honour /map?slot=N on first paint (the server already did for SSR).
      const query = new URLSearchParams(window.location.search);
      const fromQuery = query.get("slot");
      if (fromQuery) seed.selected = Number(fromQuery) || seed.selected;
      // …and `?macro=NAME` the same way, so a reload (or a no-JS tab click
      // that came back with JavaScript on) lands on the same sequence.
      const macroQuery = query.get("macro");
      if (macroQuery) seed.macro_selected = macroQuery;
      selectSlot(seed.selected);
      applyMap(seed);
      // v9: we landed on a no-JS POST's redirect (JS came back, or the user
      // reloaded that URL). The server already rendered the flash line, but
      // adoption blanks it — the client's channel is the toast stack — so
      // re-report it there ONCE and clean the URL, or a reload would replay
      // feedback for an action nobody just took.
      const flash = (query.get("flash") ?? "").trim();
      if (flash !== "") {
        pushToast(flash, { kind: flash.startsWith("error") ? "err" : "ok" });
        window.history.replaceState(
          null,
          "",
          fromQuery ? `/map?slot=${fromQuery}` : "/map",
        );
      }
    }
    wire(el);
    // The macro editor's form controls carry values an attribute binding
    // cannot keep once they are dirty; seed them from the draft right away.
    syncMacroControls();
    window.setInterval(() => void poll(), POLL_MS);
    return MapIsland();
  },
});
