import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// PadsPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { PadsPage } from "./PadsPage";
import {
  PadsIsland,
  applyFlash,
  applyPads,
  applyUnreachable,
  disarm,
  type PadsPayload,
} from "./PadsIsland";

void PadsPage; // compile-time anchor only (see above)

/** Poll cadence, same as the status page: pads appear and disappear on their
 *  own here (a spawn unplugs itself when the hold expires), so watching the
 *  list IS the feedback. */
const POLL_MS = 2000;

/** The three spawn `<select>`s, by the ids PadsIsland.ts renders. */
const SPAWN_SELECTS = ["count", "persona", "hold_secs"] as const;

/** What the user has picked in each spawn `<select>`, right now.
 *
 *  Needed because the option rows key on value AND label (the reconcile rule
 *  in PadsIsland.ts): a label that tracks the machine is a row that REBUILDS
 *  when the reading moves, and a rebuilt `<option>` list resets its `<select>`
 *  to the first entry. Without this, every poll on which the XInput occupancy
 *  or the bus headroom changed silently discarded the user's choice while
 *  they were lining up a click. render_pads.rs pins this pairing by name. */
function spawnChoices(): Map<string, string> {
  const chosen = new Map<string, string>();
  for (const id of SPAWN_SELECTS) {
    const el = document.getElementById(id) as HTMLSelectElement | null;
    if (el && el.value !== "") chosen.set(id, el.value);
  }
  return chosen;
}

/** Put the user's choice back — but only where the fresh offer still carries
 *  it. A value the backend withdrew stays withdrawn: the first option is the
 *  backend's default, and resurrecting a vanished choice would re-offer a
 *  click the provider just took off the table. */
function restoreSpawnChoices(chosen: Map<string, string>): void {
  for (const [id, value] of chosen) {
    const el = document.getElementById(id) as HTMLSelectElement | null;
    if (!el) continue;
    if (Array.from(el.options).some((o) => o.value === value)) {
      el.value = value;
    }
  }
}

async function poll(): Promise<void> {
  try {
    const payload = await fetchJSON<PadsPayload>("/api/pads");
    // Capture BEFORE applyPads: the signal writes inside it patch the lists
    // synchronously, and the reset-to-first happens as part of that patch.
    const chosen = spawnChoices();
    applyPads(payload);
    restoreSpawnChoices(chosen);
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms. With JS off they POST + 303 + full
 *  reload; with JS on the outcome is read from the redirect's ?flash= and
 *  flashed inline. Delegated on the island root so branches re-rendered by a
 *  show toggle stay wired. */
function wireForms(root: HTMLElement): void {
  root.addEventListener("submit", (ev) => {
    const form = ev.target as HTMLFormElement | null;
    if (!form || form.method.toLowerCase() !== "post") return;
    ev.preventDefault();
    void submitForm(form);
  });
}

async function submitForm(form: HTMLFormElement): Promise<void> {
  try {
    const body = new URLSearchParams();
    new FormData(form).forEach((value, key) => {
      if (typeof value === "string") body.append(key, value);
    });
    const res = await fetch(form.action, {
      method: "POST",
      body,
      redirect: "follow", // 303 → GET /pads?flash=…; the outcome rides res.url
    });
    applyFlash(new URL(res.url).searchParams.get("flash"));
    // An action DISARMS — both halves of it. The client's own arming flag goes
    // (the panel closes on the next poll), and `?confirm=1` leaves the address
    // bar, which would otherwise re-arm on a manual reload after the thing had
    // already been done.
    disarm();
    if (window.location.search !== "") {
      window.history.replaceState(null, "", "/pads");
    }
  } catch {
    applyFlash("error: request failed — is ksx studio still running?");
  }
  void poll();
}

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

activateIslands({
  // One island: the whole screen, seeded from the same PadsPayload JSON that
  // /api/pads serves.
  //
  // Order matters (docs/FORMA-DOGFOOD.md finding #5): the signals MUST hold
  // the server's values BEFORE PadsIsland() builds the descriptor tree —
  // adoption binds effects that immediately write signal state into the DOM,
  // so seeding after adoption would clobber the SSR text with defaults.
  PadsIsland: (el) => {
    const seed = embeddedPayload<PadsPayload>();
    if (seed) {
      applyPads(seed);
      applyFlash(seed.flash);
      if (seed.flash) {
        // The flash arrived via /pads?flash=…; clean the URL so a manual
        // reload does not replay stale feedback. `?confirm=1` goes with it,
        // which is correct — the action it armed has happened.
        window.history.replaceState(null, "", "/pads");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return PadsIsland();
  },
});
