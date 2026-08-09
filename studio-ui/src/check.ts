import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// CheckPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { CheckPage } from "./CheckPage";
import {
  CheckIsland,
  applyCheck,
  applyCounters,
  applyFeedState,
  applyKeys,
  type CheckPayload,
  type LiveEnvelope,
} from "./CheckIsland";

void CheckPage; // compile-time anchor only (see above)

/** How often the ROSTER is re-read. Slow on purpose: this is the binding
 *  table, which only changes when somebody edits a preset. The live half does
 *  not poll at all — it is pushed. */
const ROSTER_MS = 5000;

/** How long a chip stays lit after a HIT that is already released.
 *
 *  A press shorter than a display frame arrives in `hit` and never appears in
 *  `down`, so without this it would light for one frame — 16 ms — and be
 *  invisible to a human standing at a cabinet. 140 ms is long enough to read
 *  and short enough that two deliberate taps are still two flashes. */
const FLASH_MS = 140;

/** Keys kept in the strip. The newest are the ones somebody staring at the
 *  panel is pressing now. */
const KEY_HISTORY = 12;

/** Pending flash-off timers, keyed by the chip. A chip that fires again while
 *  still lit restarts its own timer rather than stacking a second one — which
 *  is what a turbo button does, sixty times a second.
 *
 *  A TIMER and not a deadline swept on the next frame: frames only arrive when
 *  something happens (plus a 2 s keepalive), so a swept deadline would leave a
 *  single tap lit for up to two seconds on an otherwise idle panel. */
const flashTimers = new Map<Element, number>();

/** The key strip's contents, newest last. */
let keys: { key: string; alias: string; state: string }[] = [];

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`) —
 *  not the island's `props` argument, which carries the RENDERED SLOT VALUES.
 *  See status.ts for the longer note (dogfood ledger #8/#19). */
function embeddedPayload<T>(): T | null {
  const el = document.getElementById("__ksx-payload");
  if (!el?.textContent) return null;
  try {
    return JSON.parse(el.textContent) as T;
  } catch {
    return null;
  }
}

async function pollRoster(): Promise<void> {
  try {
    const payload = await fetchJSON<CheckPayload>("/api/check");
    applyCheck(payload);
  } catch {
    // The roster is a disk read on the server; a failure here says the page's
    // own server is gone, which the feed line will also be saying.
  }
}

/** Light exactly the chips the frame says are down, and flash the ones that
 *  went down and came back inside it.
 *
 *  Direct DOM, deliberately — see CheckIsland.ts's header. The chips are found
 *  by the two RAW attributes the island renders (`data-slot`, `data-control`),
 *  so there is no id spelled in two languages to drift. */
function paint(envelope: LiveEnvelope): void {
  const grid = document.getElementById("chipgrid");
  if (!grid) return;

  // A session that has ended holds nothing. Without this, quitting the game
  // with a button pressed would leave that chip lit for as long as the tab
  // stayed open — a control shown as held on a pipeline that is not running.
  if (!envelope.frame.running) {
    clearHolds(grid, null);
  }

  for (const slot of envelope.frame.slots) {
    // **Clear the holds of THIS SLOT ONLY.**
    //
    // A frame carries a slot only when that slot published a transition, so an
    // absent slot means "nothing changed here" — NOT "nothing is pressed". The
    // first version cleared every `.down` on the grid each frame and re-applied
    // from `frame.slots`, which meant a button held on P1 went dark the instant
    // P2 pressed anything, and dark permanently on a panel where nothing else
    // moved. That is precisely the reading this screen exists to make
    // trustworthy.
    clearHolds(grid, slot.slot);
    for (const control of slot.down) {
      chipFor(grid, slot.slot, control)?.classList.add("down");
    }
    for (const control of slot.hit) {
      const chip = chipFor(grid, slot.slot, control);
      if (chip) flash(chip);
    }
  }
}

/** Drop the held state of one slot, or of every slot when `slot` is null. */
function clearHolds(grid: HTMLElement, slot: number | null): void {
  const selector =
    slot === null
      ? ".chip.down"
      : `.chip.down[data-slot="${CSS.escape(String(slot))}"]`;
  for (const chip of Array.from(grid.querySelectorAll(selector))) {
    chip.classList.remove("down");
  }
}

/** Light a chip for [`FLASH_MS`], restarting the clock if it is already lit. */
function flash(chip: Element): void {
  const pending = flashTimers.get(chip);
  if (pending !== undefined) window.clearTimeout(pending);
  chip.classList.add("flash");
  flashTimers.set(
    chip,
    window.setTimeout(() => {
      chip.classList.remove("flash");
      flashTimers.delete(chip);
    }, FLASH_MS),
  );
}

/** One chip, by the values the payload carried. `CSS.escape` because a control
 *  name is data — `dpad.up` has a dot in it, and a selector that took it
 *  literally would ask for a class. */
function chipFor(grid: HTMLElement, slot: number, control: string): Element | null {
  return grid.querySelector(
    `.chip[data-slot="${CSS.escape(String(slot))}"][data-control="${CSS.escape(control)}"]`,
  );
}

/** The key strip. `down` presses are what somebody wants to see; releases are
 *  noise on a screen read at arm's length. */
function pushKeys(envelope: LiveEnvelope): void {
  const arrived = envelope.frame.keys.filter((k) => k.down);
  if (arrived.length === 0) return;
  for (const hit of arrived) {
    keys.push({
      key: hit.key,
      // The daemon names devices (it can read the config; the engine thread
      // cannot). An unnamed board stays unnamed rather than being guessed at.
      alias: hit.alias,
      // Part of the list key, so two presses of the same key on the same board
      // are two rows rather than one that silently does not re-render.
      state: String(Date.now()) + ":" + String(keys.length),
    });
  }
  keys = keys.slice(-KEY_HISTORY);
  applyKeys(keys);
}

/** Both loss counters, in words, SHOWN. `dropped` is what this consumer missed
 *  because it could not keep up; `off_panel` is keys from a board bound to no
 *  slot. They are different findings and they get different sentences — "the
 *  panel is dead" and "you are pressing the wrong keyboard" must never look
 *  the same. */
function counters(envelope: LiveEnvelope): void {
  const dropped = envelope.frame.dropped;
  const off = envelope.frame.off_panel;
  applyCounters(
    dropped > 0
      ? `${dropped} event(s) were dropped on the way here — this page could not ` +
          `keep up, so something you pressed may not be shown.`
      : "",
    off > 0
      ? `${off} key press(es) came from a keyboard assigned to no player and were left ` +
          `out. If nothing is lighting up, check you are pressing the panel.`
      : "",
  );
}

export function customerFeedReason(reason: string | null | undefined): string {
  if (!reason) return "live";
  const lower = reason.toLowerCase();
  if (lower.includes("daemon") || lower.includes("control channel") || lower.includes("pipe")) {
    return "Live testing needs ksx to be reopened.";
  }
  if (lower.includes("no session") || lower.includes("not running")) {
    return "Press Play in Setup to start live testing.";
  }
  // Refusals originate below the presentation boundary and may contain pipe
  // names, commands, paths, or other support detail. An unfamiliar one still
  // gets a useful customer action; its raw text never becomes primary copy.
  return "Live testing is temporarily unavailable. Reopen ksx and try again.";
}

/** One EventSource for the page's life. It reconnects by itself, on the
 *  interval the SERVER sets (`retry:` — ksx-studio/src/live.rs), so there is
 *  no backoff to get right here and none is written. */
function connect(): void {
  const source = new EventSource("/api/live");

  source.addEventListener("open", () => {
    applyFeedState("connected — waiting for input", true);
  });

  source.addEventListener("frame", (ev) => {
    let envelope: LiveEnvelope;
    try {
      envelope = JSON.parse((ev as MessageEvent<string>).data) as LiveEnvelope;
    } catch {
      applyFeedState("live input sent something this page could not read", false);
      return;
    }
    // The provider owns the reason, but it crosses a customer presentation
    // boundary before it is shown: raw paths, commands and channel names stay
    // in support detail rather than becoming this screen's status line.
    const reason = envelope.unavailable;
    applyFeedState(customerFeedReason(reason), !reason);
    counters(envelope);
    pushKeys(envelope);
    paint(envelope);
  });

  source.addEventListener("unavailable", (ev) => {
    let refusal: { message?: string; remedy?: string | null } = {};
    try {
      refusal = JSON.parse((ev as MessageEvent<string>).data) as typeof refusal;
    } catch {
      // fall through to the generic sentence below
    }
    const reason = refusal.message?.trim() || refusal.remedy?.trim() || "unavailable";
    applyFeedState(customerFeedReason(reason), false);
  });

  // `error` on an EventSource is not fatal — the browser is already
  // reconnecting on the server's `retry:` interval. Saying "reconnecting"
  // rather than "failed" is the honest word for what is actually happening.
  source.addEventListener("error", () => {
    applyFeedState("reconnecting to live input…", false);
  });
}

activateIslands({
  // One island: the whole screen, seeded from the same CheckPayload JSON that
  // /api/check serves.
  //
  // Order matters (docs/FORMA-DOGFOOD.md finding #5): the signals MUST hold the
  // server's values BEFORE CheckIsland() builds the descriptor tree — adoption
  // binds effects that immediately write signal state into the DOM, so seeding
  // after adoption would clobber the SSR text with defaults.
  CheckIsland: () => {
    const seed = embeddedPayload<CheckPayload>();
    if (seed) applyCheck(seed);
    applyKeys([]);
    applyFeedState("connecting to live input…", false);
    applyCounters("", "");
    connect();
    window.setInterval(() => void pollRoster(), ROSTER_MS);
    return CheckIsland();
  },
});
