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

async function poll(): Promise<void> {
  try {
    applyPads(await fetchJSON<PadsPayload>("/api/pads"));
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
