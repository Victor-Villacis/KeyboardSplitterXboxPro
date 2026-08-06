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
  macroIsDirty,
  macroRename,
  macroSeededFrom,
  macroSetAllowShort,
  macroSetDuration,
  macroSetPolicy,
  macroSetUnit,
  macroStepAllowShort,
  macroStepUnit,
  macroStepVerb,
  macroToggleCell,
  macroTomlText,
  markPaused,
  markSaved,
  modalIsOpen,
  newestUndoable,
  previousKeys,
  currentMacro,
  currentMacroStep,
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
  setMultiMode,
  showConflict,
  showLearnMode,
  showListening,
  toggleSelected,
  updateCountdown,
  writableKeys,
  type BindOutcome,
  type LearnView,
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
    applyMap(await fetchJSON<MapPayload>("/api/map"));
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
): Promise<BindOutcome> {
  try {
    return await fetchJSON<BindOutcome>("/api/bind/keys", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ preset, function: fn, keys, force, reload: true }),
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

// ── v11: the macro editor's controls ───────────────────────────────────────
// The grid edits a DRAFT (docs/INPUT-TRANSFORMS.md §1c keeps step authoring in
// TOML — no daemon verb writes a step list), so nothing here calls a write
// verb. What it does call is `syncMacroControls`: three <select>s, a number
// input, a checkbox and a text input whose VALUES cannot come from an
// attribute binding once the user has touched them (a dirty form control
// ignores its attribute), so the island exposes the draft's truth and this
// writes it onto the DOM after every edit and every poll.

let islandRoot: HTMLElement | null = null;

function syncMacroControls(): void {
  const root = islandRoot;
  const mac = currentMacro();
  if (!root || !mac) return;
  const set = (sel: string, value: string): void => {
    const el = root.querySelector<HTMLInputElement | HTMLSelectElement>(sel);
    if (el && el.value !== value) el.value = value;
  };
  set(".macdurin", String(macroDurationValue()));
  set(".macunit", macroStepUnit());
  set(".macnamein", mac.name);
  set('.macsel[data-macpol="on_release"]', mac.on_release);
  set('.macsel[data-macpol="retrigger"]', mac.retrigger);
  set('.macsel[data-macpol="interrupt"]', mac.interrupt);
  const short = root.querySelector<HTMLInputElement>(".macshortin");
  if (short) short.checked = macroStepAllowShort();
}

/** The number the duration box should show for the selected step, in the unit
 *  it was authored in. */
function macroDurationValue(): number {
  const at = currentMacroStep();
  const step = at === null ? undefined : currentMacro()?.steps[at];
  if (!step) return 50;
  return step.frames ?? step.ms ?? 50;
}

function macroCurrentUnit(): string {
  const el = islandRoot?.querySelector<HTMLSelectElement>(".macunit");
  return el?.value === "frames" ? "frames" : "ms";
}

/** One `verb|index` payload from a row button. */
function macroAct(payload: string): void {
  const at = payload.indexOf("|");
  if (at <= 0) return;
  macroStepVerb(payload.slice(0, at), Number(payload.slice(at + 1)));
  syncMacroControls();
}

/** One `stepIndex|function` payload from a grid cell. */
function macroCell(payload: string): void {
  const at = payload.indexOf("|");
  if (at < 0) return;
  macroToggleCell(Number(payload.slice(0, at)), payload.slice(at + 1));
  syncMacroControls();
}

/** Switch the editor to another macro — or to a fresh sequence. A draft with
 *  unsaved edits is not silently thrown away: it says so, and the TOML block
 *  it produced was on screen the whole time. */
function macroSwitch(name: string | null): void {
  if (macroIsDirty()) {
    pushToast(
      "That draft was not saved anywhere — macro steps are pasted into the preset as TOML, " +
        "and this one is gone. (Nothing on disk changed.)",
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
    pushToast("The [macros] block is on the clipboard — paste it into the preset file.");
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
    // ── v11: macro-card verbs. None of them writes anything — the output is
    // the TOML block (§1c: step authoring stays TOML-only). ───────────────
    if (act === "macro-new") {
      macroSwitch(null);
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
    if (el.classList.contains("macdurin")) {
      if (committed) macroSetDuration(Number(el.value), macroCurrentUnit());
      return;
    }
    if (el.classList.contains("macunit")) {
      // CONVERTS, never reinterprets: 50 ms picked as frames is 3 frames, not
      // 50 of them. The unit is an authoring convenience, so changing it must
      // not change how long the step runs.
      macroSetUnit(el.value);
      syncMacroControls();
      return;
    }
    if (el instanceof HTMLInputElement && el.classList.contains("macshortin")) {
      macroSetAllowShort(el.checked);
      return;
    }
    if (el.classList.contains("macnamein")) {
      if (committed) macroRename(el.value);
      return;
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
    const el = (ev.target as HTMLElement | null)?.closest<HTMLElement>("[data-fn]");
    setHot(el?.dataset.fn ?? null);
  };
  root.addEventListener("mouseover", hotFrom);
  root.addEventListener("focusin", hotFrom);
  root.addEventListener("mouseleave", () => setHot(null));

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

activateIslands({
  // Ledger #5 order: signals seeded from the props BEFORE adoption.
  MapIsland: (el, props) => {
    if (props) {
      const seed = props as unknown as MapPayload;
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
