import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// StartPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { StartPage } from "./StartPage";
import {
  StartIsland,
  applyFlash,
  applyStart,
  applyUnreachable,
  type StartPayload,
} from "./StartIsland";

void StartPage; // compile-time anchor only (see above)

/** Poll cadence. The device list is the reason: a first-run user plugs their
 *  keyboard in WHILE this page is open, and FIRST-RUN.md §5 says they must not
 *  have to know a scan exists. Every poll is a fresh enumeration server-side
 *  (`collect_start` never caches), so the board simply appears. The Rescan
 *  button is the same read, made visible for the user who is watching. */
const POLL_MS = 2000;

/** What the user has picked in the persona `<select>`, right now.
 *
 *  Needed for the reason pads.ts documents: the option rows key on value AND
 *  label, so a relabelled option is a REBUILT option, and a rebuilt `<option>`
 *  list resets its `<select>` to the first entry. Without this, a poll that
 *  landed while someone was lining up a click would silently discard their
 *  choice. render_start.rs pins the pairing by name. */
function personaChoice(): string | null {
  const el = document.getElementById("persona") as HTMLSelectElement | null;
  return el && el.value !== "" ? el.value : null;
}

function restorePersonaChoice(chosen: string | null): void {
  if (chosen === null) return;
  const el = document.getElementById("persona") as HTMLSelectElement | null;
  if (!el) return;
  // A value the backend WITHDREW stays withdrawn: the first option is the
  // backend's own default, and resurrecting a vanished choice would re-offer a
  // persona the roster just stopped being able to plug.
  if (Array.from(el.options).some((o) => o.value === chosen)) {
    el.value = chosen;
  }
}

async function poll(): Promise<void> {
  try {
    const payload = await fetchJSON<StartPayload>("/api/start");
    // Capture BEFORE applyStart: the signal writes inside it patch the lists
    // synchronously, and the reset-to-first happens as part of that patch.
    const chosen = personaChoice();
    applyStart(payload);
    restorePersonaChoice(chosen);
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms. With JS off they POST + 303 + full
 *  reload; with JS on the outcome is read from the redirect's ?flash= and
 *  flashed inline, so the page never navigates and a user keeps their place in
 *  a four-step flow. Delegated on the island root so branches re-rendered by a
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
      redirect: "follow", // 303 → GET /start?flash=…; the outcome rides res.url
    });
    applyFlash(new URL(res.url).searchParams.get("flash"));
    if (window.location.search !== "") {
      window.history.replaceState(null, "", "/start");
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
  // One island: the whole screen, seeded from the same StartPayload JSON that
  // /api/start serves.
  //
  // Order matters (docs/FORMA-DOGFOOD.md finding #5): the signals MUST hold the
  // server's values BEFORE StartIsland() builds the descriptor tree — adoption
  // binds effects that immediately write signal state into the DOM, so seeding
  // after adoption would clobber the SSR text with defaults.
  StartIsland: (el) => {
    const seed = embeddedPayload<StartPayload>();
    if (seed) {
      applyStart(seed);
      applyFlash(seed.flash);
      if (seed.flash) {
        // The flash arrived via /start?flash=…; clean the URL so a manual
        // reload does not replay stale feedback about a save that happened
        // once.
        window.history.replaceState(null, "", "/start");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return StartIsland();
  },
});
