import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// DevicesPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { DevicesPage } from "./DevicesPage";
import {
  DevicesIsland,
  applyDevices,
  applyFlash,
  applyUnreachable,
  type DevicesPayload,
} from "./DevicesIsland";

void DevicesPage; // compile-time anchor only (see above)

/** Poll cadence, matching the status page. USB enumeration is the expensive
 *  half and it is re-run every time on purpose (sources.rs: "a status row that
 *  is quietly ten minutes old is worse than one that took 40 ms to fetch") —
 *  a board that has just been unplugged must stop being offered. */
const POLL_MS = 2000;

async function poll(): Promise<void> {
  try {
    applyDevices(await fetchJSON<DevicesPayload>("/api/devices"));
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms. With JavaScript off they POST + 303 +
 *  full reload, which is the baseline this page is built on; with it on, the
 *  submit goes through fetch, the outcome is read out of the redirect's
 *  ?flash= query, and the lists refresh in place — so the row you just acted
 *  on is still under the cursor. Delegated on the island root, because both
 *  lists are reconciled and a per-form listener would die with its row. */
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
      redirect: "follow", // 303 → GET /devices?flash=…; the outcome rides res.url
    });
    applyFlash(new URL(res.url).searchParams.get("flash"));
  } catch {
    applyFlash("error: request failed — is ksx studio still running?");
  }
  void poll();
}

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`).
 *  Deliberately NOT the island's `props` argument — since compiler 0.3.1
 *  populates island `slot_ids`, `props` carries the RENDERED SLOT VALUES, and
 *  this page derives its rows from the payload instead. Ledger #19. */
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
  // One island: the whole screen, seeded from the same DevicesPayload JSON
  // that /api/devices serves.
  //
  // Order matters (docs/FORMA-DOGFOOD.md finding #5): the signals MUST hold
  // the server's values BEFORE DevicesIsland() builds the descriptor tree —
  // adoption binds effects that immediately write signal state into the DOM,
  // so seeding after adoption would clobber the SSR text with defaults.
  DevicesIsland: (el) => {
    const seed = embeddedPayload<DevicesPayload>();
    if (seed) {
      applyDevices(seed);
      applyFlash(seed.flash);
      if (seed.flash && window.location.search !== "") {
        // The flash arrived via /devices?flash=…; clean the URL so a manual
        // reload does not replay stale feedback.
        window.history.replaceState(null, "", "/devices");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return DevicesIsland();
  },
});
