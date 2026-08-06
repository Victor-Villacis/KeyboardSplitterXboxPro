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
  flashSaved,
  identityLabel,
  isMultiMode,
  isPaused,
  learnAllowed,
  liveProfile,
  markPaused,
  markSaved,
  modalIsOpen,
  profileToResume,
  selectFn,
  selectSlot,
  selectedFnName,
  selectedFns,
  selectionCount,
  setHot,
  setMultiMode,
  showConflict,
  showListening,
  toggleSelected,
  updateCountdown,
  type BindOutcome,
  type LearnView,
  type MapPayload,
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
  selectFn(fns[0]);
  armFocusGuard();
  try {
    const started = await fetchJSON<LearnView>("/api/learn/start", { method: "POST" });
    if (learnGen !== gen) return; // superseded while the POST was in flight
    if (started.state !== "listening") {
      learnTargets = [];
      disarmFocusGuard();
      flashSaved(`error: ${started.error ?? `learn refused (${started.state})`}`, true);
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
    flashSaved("error: request failed — is ksx studio still running?", true);
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
        if (targets.length === 1) await saveBinding(targets[0], learn.key, false);
        else await mapAll(targets, learn.key);
      }
      break;
    case "timeout":
      stopLearnTimer();
      learnTargets = [];
      disarmFocusGuard();
      closeModal();
      flashSaved(`timed out — no key pressed within 10 s for ${names}`, true);
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
      flashSaved(`error: ${learn.error ?? `learn ${learn.state}`}`, true);
      break;
  }
}

async function cancelLearn(): Promise<void> {
  stopLearnTimer();
  learnTargets = [];
  learnGen += 1;
  pendingKey = null;
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

/** Write one binding — `key: null` CLEARS it (the `ksx map --clear` verb, same
 *  writer, no GUI-only path). */
async function saveBinding(fn: string, key: string | null, force: boolean): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  const outcome = await bindOnce(slot.preset, fn, key, force);
  if (outcome.ok) {
    closeModal();
    pendingKey = null;
    markSaved();
    let line = outcome.message ?? (key === null ? `${fn} cleared` : `${fn} = ${key}`);
    if (isPaused()) line += " — Resume emulation when you're done";
    flashSaved(line, false);
  } else if (outcome.code === "conflict" && outcome.conflicts.length > 0) {
    // FEATURE 3. A key already used by ANOTHER CONTROL IN THIS PRESET is a
    // multi-bind, not an error: the engine compiles one key to several targets
    // and applies them all (docs/INPUT-TRANSFORMS.md §1a). So no dialog, no
    // "Replace" — write it and let the flash carry the daemon's own sentence.
    // The legend's "also …" badges then re-derive from disk on the next poll,
    // so the page shows what actually happened rather than what we assumed.
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
    flashSaved(`error: ${outcome.error ?? "the daemon refused the write"}`, true);
  }
  void poll(); // zone tags refresh from disk truth
}

/** FEATURE 2's write: the captured key goes to EVERY selected control as N
 *  ordinary `map` calls — which is exactly what a multi-bind is in the preset
 *  file (`A = "P"`, `B = "P"`, `rt = "P"`). `force` is set because the second
 *  and later writes see the first as a same-preset "conflict", which here is
 *  the intent. Sequential on purpose: the writer is one file, and a partial
 *  result must be reportable control by control. */
async function mapAll(fns: string[], key: string): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  flashSaved(`binding ${fns.length} controls to ${key}…`, false);
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
    line = `${key} now drives ${kept.map(identityLabel).join(" · ")}`;
    if (isPaused()) line += " — Resume emulation when you're done";
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
  flashSaved(line, bad);
  clearSelection();
}

/** Clear every selected control in one action (the selection bar's second
 *  button). Confirms first — this is N destructive writes, and MAPPER-UX
 *  commandment 5 says a whole-group write states what it will do. */
async function clearSelectedBindings(): Promise<void> {
  const fns = selectedFns();
  if (fns.length === 0) return;
  if (!learnAllowed()) {
    refuseSelection();
    return;
  }
  const names = fns.map(identityLabel).join(", ");
  const question =
    `Clear the binding on ${fns.length} control${fns.length === 1 ? "" : "s"}?\n\n` +
    `${names}\n\n` +
    "They stay listed and unbound; nothing else in the preset is touched.";
  if (!window.confirm(question)) return;
  const slot = currentSlot();
  if (!slot) return;
  const done: string[] = [];
  const failed: string[] = [];
  for (const fn of fns) {
    const outcome = await bindOnce(slot.preset, fn, null, false);
    if (outcome.ok) done.push(identityLabel(fn));
    else failed.push(`${identityLabel(fn)} (${outcome.error ?? outcome.code ?? "refused"})`);
  }
  if (done.length > 0) markSaved();
  let line = done.length > 0 ? `cleared ${done.join(" · ")}` : "nothing was cleared";
  if (failed.length > 0) line += ` — FAILED: ${failed.join("; ")}`;
  flashSaved(line, failed.length > 0);
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
  flashSaved(`can't learn ${fn} — ${reason}. From a shell: ${cli}`, true);
}

/** The same answer for an action that is about a SELECTION, not one control. */
function refuseSelection(): void {
  flashSaved(
    `can't map right now — ${blockedReason() ?? "mapping is unavailable"}`,
    true,
  );
}

// ── FIX 0: pause / resume, so the refusal is one click, not a dead end ─────

async function pauseAndMap(): Promise<void> {
  const profile = liveProfile();
  flashSaved("pausing emulation…", false);
  const out = await verb("/api/session/stop");
  if (out.ok) {
    markPaused(profile);
    flashSaved(
      `emulation paused${profile ? ` ("${profile}")` : ""} — map away, then Resume emulation`,
      false,
    );
  } else {
    flashSaved(`error: ${out.error ?? "the daemon refused to stop"}`, true);
  }
  void poll();
}

async function resumeEmulation(): Promise<void> {
  const profile = profileToResume();
  flashSaved("resuming emulation…", false);
  const out = await verb("/api/session/start", profile ? { profile } : {});
  if (out.ok) {
    clearPaused();
    flashSaved(out.message ?? "emulation resumed", false);
  } else {
    flashSaved(`error: ${out.error ?? "the daemon refused to start"}`, true);
  }
  void poll();
}

// ── Preset-level writes (restore ×3, clear all) ────────────────────────────
// Every one of them confirms first, and the confirm states exactly what will
// be WRITTEN and what is BACKED UP before it — MAPPER-UX commandment 5.

type RestoreMode = "defaults" | "session-backup" | "latest-backup";

function restoreQuestion(mode: RestoreMode, preset: string): string {
  const tail =
    "\n\nThe current file is copied to <preset>.toml.bak-YYYYMMDD-HHMMSS first, " +
    'so this is undoable with "Restore backup from …".';
  switch (mode) {
    case "defaults":
      return (
        `Reset "${preset}" to the GENERIC KEYBOARD layout?\n\n` +
        "This writes S=A, D=B, A=X, W=Y, Q/E triggers, arrow keys = left stick, " +
        "Esc=Start — a desktop keyboard layout. It is NOT this preset's original " +
        "panel map; every binding you see now is replaced." +
        tail
      );
    case "session-backup":
      return (
        `Undo this session's changes to "${preset}"?\n\n` +
        "This writes the preset as it was before the daemon's first change since " +
        "it started." +
        tail
      );
    case "latest-backup":
      return (
        `Restore "${preset}" from its newest timestamped backup?\n\n` +
        "This writes the preset as it was before the most recent restore." +
        tail
      );
  }
}

async function restorePreset(mode: RestoreMode): Promise<void> {
  const preset = currentPreset();
  if (!preset) return;
  if (!window.confirm(restoreQuestion(mode, preset))) return;
  const out = await verb("/api/preset/restore", { preset, mode });
  if (out.ok) markSaved();
  flashSaved(out.ok ? (out.message ?? "restored") : `error: ${out.error ?? "the daemon refused"}`, !out.ok);
  void poll();
}

async function clearAll(): Promise<void> {
  const preset = currentPreset();
  if (!preset) return;
  const question =
    `Clear EVERY binding in "${preset}"?\n\n` +
    "All 25 controls stay listed but none of them will be bound — the slot's pad " +
    "stops responding to the panel until you map it again.\n\n" +
    "The current file is copied to <preset>.toml.bak-YYYYMMDD-HHMMSS first, so " +
    'this is undoable with "Restore backup from …".';
  if (!window.confirm(question)) return;
  const out = await verb("/api/preset/clear-all", { preset });
  if (out.ok) markSaved();
  flashSaved(out.ok ? (out.message ?? "cleared") : `error: ${out.error ?? "the daemon refused"}`, !out.ok);
  void poll();
}

// ── Wiring: delegated events on the island root ────────────────────────────

function wire(root: HTMLElement): void {
  // Multi-select is a JS enhancement: the "Select multiple" toggle stays
  // hidden until this class exists, so a no-JS page never shows a control that
  // cannot do anything (the whole page's standing rule — FIX 1).
  root.classList.add("js");

  root.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement | null;
    if (!target) return;

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
      void cancelLearn();
      selectSlot(Number(tab.dataset.slot));
      return;
    }

    const zone = target.closest<HTMLElement>("[data-fn]");
    if (zone?.dataset.fn) {
      const fn = zone.dataset.fn;
      // Victor's file-explorer analogy: Ctrl/Shift/⌘-click ADDS to a
      // selection — and on touch, where no modifier exists, the header's
      // "Select multiple" toggle makes every plain tap do the same.
      const additive = ev.ctrlKey || ev.metaKey || ev.shiftKey || isMultiMode();
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
      const fromQuery = new URLSearchParams(window.location.search).get("slot");
      if (fromQuery) seed.selected = Number(fromQuery) || seed.selected;
      selectSlot(seed.selected);
      applyMap(seed);
    }
    wire(el);
    window.setInterval(() => void poll(), POLL_MS);
    return MapIsland();
  },
});
