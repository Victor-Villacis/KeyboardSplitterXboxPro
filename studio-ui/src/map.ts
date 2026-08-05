import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";
// Compile-time anchor: the imported *Page component NOT in the
// activateIslands registry is this entry's SSR root (see status.ts).
import { MapPage } from "./MapPage";
import {
  MapIsland,
  applyMap,
  applyMapUnreachable,
  closeModal,
  currentSlot,
  flashSaved,
  learnAllowed,
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

async function poll(): Promise<void> {
  try {
    applyMap(await fetchJSON<MapPayload>("/api/map"));
  } catch {
    applyMapUnreachable();
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

async function startLearn(fn: string): Promise<void> {
  // PadForge convention: clicking the control being recorded cancels it.
  if (learningFn === fn) {
    await cancelLearn();
    return;
  }
  learningFn = fn;
  pendingKey = null;
  selectFn(fn);
  try {
    const started = await fetchJSON<LearnView>("/api/learn/start", { method: "POST" });
    if (started.state !== "listening") {
      learningFn = null;
      flashSaved(`error: ${started.error ?? `learn refused (${started.state})`}`, true);
      return;
    }
    showListening(prompt(fn), started.remaining_ms ?? LEARN_TOTAL_MS, LEARN_TOTAL_MS);
    stopLearnTimer();
    learnTimer = window.setInterval(() => void pollLearn(), LEARN_POLL_MS);
  } catch {
    learningFn = null;
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
      closeModal();
      if (learn.key) {
        await saveBinding(fn, learn.key, false);
      }
      break;
    case "timeout":
      stopLearnTimer();
      learningFn = null;
      closeModal();
      flashSaved(`timed out — no key pressed within 10 s for ${fn}`, true);
      break;
    case "cancelled":
      stopLearnTimer();
      learningFn = null;
      closeModal();
      break;
    default:
      // failed / unavailable / idle-after-restart: report and stop.
      stopLearnTimer();
      learningFn = null;
      closeModal();
      flashSaved(`error: ${learn.error ?? `learn ${learn.state}`}`, true);
      break;
  }
}

async function cancelLearn(): Promise<void> {
  stopLearnTimer();
  learningFn = null;
  pendingKey = null;
  closeModal();
  try {
    await fetch("/api/learn/cancel", { method: "POST" });
  } catch {
    // the modal is already closed; a lost cancel just times out server-side
  }
}

async function saveBinding(fn: string, key: string, force: boolean): Promise<void> {
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
        reload: true, // a running session bounces onto the new binding
      }),
    });
    if (outcome.ok) {
      closeModal();
      pendingKey = null;
      flashSaved(outcome.message ?? `${fn} = ${key}`, false);
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

// ── Wiring: delegated events on the island root ────────────────────────────

function wire(root: HTMLElement): void {
  root.addEventListener("click", (ev) => {
    const target = ev.target as HTMLElement | null;
    if (!target) return;

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
        // Read-only: the click still selects the control, prefilled into
        // the CLI fallback line.
        selectFn(fn);
      }
    }
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
    } else if (ev.key === "Escape") {
      closeModal();
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
