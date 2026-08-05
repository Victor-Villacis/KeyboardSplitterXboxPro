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
  closeModal,
  currentBinding,
  currentPreset,
  currentSlot,
  flashSaved,
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
  setHot,
  showConflict,
  showListening,
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

let learningFn: string | null = null;
let learnTimer: number | undefined;
/** The hit waiting on the conflict dialog's verdict. */
let pendingKey: string | null = null;

function prompt(fn: string): string {
  const slot = currentSlot();
  return slot ? `Press the panel key for P${slot.number} · ${fn}` : `Press the panel key for ${fn}`;
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
  if (learningFn === null) return;
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

async function startLearn(fn: string): Promise<void> {
  // PadForge convention: clicking the control being recorded cancels it.
  if (learningFn === fn) {
    await cancelLearn();
    return;
  }
  learningFn = fn;
  pendingKey = null;
  selectFn(fn);
  armFocusGuard();
  try {
    const started = await fetchJSON<LearnView>("/api/learn/start", { method: "POST" });
    if (started.state !== "listening") {
      learningFn = null;
      disarmFocusGuard();
      flashSaved(`error: ${started.error ?? `learn refused (${started.state})`}`, true);
      return;
    }
    showListening(
      prompt(fn),
      currentBinding(fn),
      started.remaining_ms ?? LEARN_TOTAL_MS,
      LEARN_TOTAL_MS,
    );
    stopLearnTimer();
    learnTimer = window.setInterval(() => void pollLearn(), LEARN_POLL_MS);
  } catch {
    learningFn = null;
    disarmFocusGuard();
    flashSaved("error: request failed — is ksx studio still running?", true);
  }
}

async function pollLearn(): Promise<void> {
  const fn = learningFn;
  if (fn === null) {
    stopLearnTimer();
    return;
  }
  let learn: LearnView;
  try {
    learn = await fetchJSON<LearnView>("/api/learn");
  } catch {
    return; // transient — keep the countdown running on the last known value
  }
  if (learningFn !== fn) return; // superseded meanwhile
  switch (learn.state) {
    case "listening":
      updateCountdown(learn.remaining_ms ?? 0, LEARN_TOTAL_MS);
      break;
    case "hit":
      stopLearnTimer();
      learningFn = null;
      disarmFocusGuard();
      closeModal();
      if (learn.key) {
        await saveBinding(fn, learn.key, false);
      }
      break;
    case "timeout":
      stopLearnTimer();
      learningFn = null;
      disarmFocusGuard();
      closeModal();
      flashSaved(`timed out — no key pressed within 10 s for ${fn}`, true);
      break;
    case "cancelled":
      stopLearnTimer();
      learningFn = null;
      disarmFocusGuard();
      closeModal();
      break;
    default:
      // failed / unavailable / idle-after-restart: report and stop.
      stopLearnTimer();
      learningFn = null;
      disarmFocusGuard();
      closeModal();
      flashSaved(`error: ${learn.error ?? `learn ${learn.state}`}`, true);
      break;
  }
}

async function cancelLearn(): Promise<void> {
  stopLearnTimer();
  learningFn = null;
  pendingKey = null;
  disarmFocusGuard();
  closeModal();
  try {
    await fetch("/api/learn/cancel", { method: "POST" });
  } catch {
    // the modal is already closed; a lost cancel just times out server-side
  }
}

/** Write one binding — `key: null` CLEARS it (the `ksx map --clear` verb, same
 *  writer, no GUI-only path). */
async function saveBinding(fn: string, key: string | null, force: boolean): Promise<void> {
  const slot = currentSlot();
  if (!slot) return;
  try {
    const outcome = await fetchJSON<BindOutcome>("/api/bind", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        preset: slot.preset,
        function: fn,
        key,
        force,
        // A binding-only edit is hot-swapped into a running session: the pads
        // stay plugged (crates/ksx-app/src/daemon/mod.rs `apply_bindings`).
        reload: true,
      }),
    });
    if (outcome.ok) {
      closeModal();
      pendingKey = null;
      markSaved();
      let line = outcome.message ?? (key === null ? `${fn} cleared` : `${fn} = ${key}`);
      if (isPaused()) line += " — Resume emulation when you're done";
      flashSaved(line, false);
    } else if (outcome.code === "conflict") {
      // The caller decides (the PadForge gap this closes): show what owns
      // the key, offer Replace / Cancel.
      pendingKey = key;
      const lines = outcome.conflicts.map((c) =>
        c.scope === "preset"
          ? `${key} is already this preset's ${c.function}`
          : `${key} is "${c.preset}"'s ${c.function}` +
            (c.profile ? ` (slot ${c.slot} of "${c.profile}")` : ""),
      );
      showConflict(
        prompt(fn).replace("Press the panel key for", "Bind") + ` = ${key}?`,
        lines.join("; ") +
          " — Replace binds it here anyway (same-preset conflicts are stolen; other presets are never edited).",
      );
    } else {
      closeModal();
      flashSaved(`error: ${outcome.error ?? "the daemon refused the write"}`, true);
    }
  } catch {
    closeModal();
    flashSaved("error: bind request failed — is ksx studio still running?", true);
  }
  void poll(); // zone tags refresh from disk truth
}

/** Clear one control. Reached three ways — the modal's button, the legend's
 *  ✕, and Delete/Backspace while the modal is open — all landing here. */
async function clearBinding(fn: string): Promise<void> {
  if (!learnAllowed()) {
    refuse(fn);
    return;
  }
  if (learningFn !== null) await cancelLearn();
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
      if (learnAllowed()) {
        void startLearn(fn);
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
    if (ev.key === "Escape" && learningFn !== null) {
      void cancelLearn();
      return;
    }
    if (ev.key === "Escape") {
      closeModal();
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
