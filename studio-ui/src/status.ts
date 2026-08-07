import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// Dogfood ledger #13(b) closed at @getforma/core 2.0.0 — see map.ts. The
// `__ksxShowBranch` install and the build.mjs bundle patch are both gone.
//
// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// StatusPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { StatusPage } from "./StatusPage";
import {
  StatusIsland,
  applyFlash,
  applyStatus,
  applyUnreachable,
  type StatusPayload,
} from "./StatusIsland";

void StatusPage; // compile-time anchor only (see above)

/** Poll cadence. The v3 page reloaded wholesale every 5 s; v4 rewrites the
 *  same signals in place every 2 s — no reload, dropdown state survives. */
const POLL_MS = 2000;

async function poll(): Promise<void> {
  try {
    applyStatus(await fetchJSON<StatusPayload>("/api/status"));
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms (progressive enhancement: with JS off
 *  they POST + 303 + full reload exactly as in v3). Submit via fetch, read
 *  the outcome from the redirect's ?flash= query, flash it inline, and
 *  refresh the status immediately. Delegated on the island root so rows
 *  re-rendered by list reconcile stay wired. */
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
      redirect: "follow", // 303 → GET /?flash=…; the outcome rides res.url
    });
    applyFlash(new URL(res.url).searchParams.get("flash"));
  } catch {
    applyFlash("error: request failed — is ksx studio still running?");
  }
  void poll();
}

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`).
 *
 *  Deliberately NOT the island's `props` argument. Since compiler 0.3.1
 *  populates island `slot_ids`, forma-ir emits `data-forma-props` itself and
 *  `loadIslandProps` prefers it — so `props` is now the RENDERED SLOT VALUES
 *  (`vigemLine`, `show:canStart`, `list:padTiles:array`, …), not the payload
 *  this page derives everything from. Dogfood ledger #8 is closed by that
 *  (we no longer hand-emit `__forma_islands`); #19 records the gap it left. */
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
  // One island: the whole screen, seeded from the same StatusPayload JSON
  // that /api/status serves.
  //
  // Order matters (docs/FORMA-DOGFOOD.md finding #5): the signals MUST hold
  // the server's values BEFORE StatusIsland() builds the descriptor tree —
  // adoption binds effects that immediately write signal state into the DOM,
  // so seeding after adoption would clobber the SSR text with defaults.
  StatusIsland: (el) => {
    const seed = embeddedPayload<StatusPayload>();
    if (seed) {
      applyStatus(seed);
      applyFlash(seed.flash);
      if (seed.flash && window.location.search !== "") {
        // The flash arrived via /?flash=…; clean the URL so a manual reload
        // does not replay stale feedback (v3 relied on the meta refresh).
        window.history.replaceState(null, "", "/");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return StatusIsland();
  },
});
