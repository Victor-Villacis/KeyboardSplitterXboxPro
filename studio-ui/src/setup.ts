import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The /setup entry. Same shape as status.ts — read its comments for the
// islands-protocol details; what follows is what differs here.
//
// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// SetupPage never runs in the browser, but this import is what anchors IR
// emission. Do not remove it.
import { SetupPage } from "./SetupPage";
import {
  SetupIsland,
  applyFlash,
  applySetup,
  applyUnreachable,
  type SetupPayload,
} from "./SetupIsland";

void SetupPage; // compile-time anchor only (see above)

/** Poll cadence, matching the status page. It matters more here than it looks:
 *  step 3 is "press a button and watch it land", and the daemon's learner is
 *  read back by this poll — 2 s is what makes a press feel like it registered
 *  rather than like nothing happened. */
const POLL_MS = 2000;

async function poll(): Promise<void> {
  try {
    applySetup(await fetchJSON<SetupPayload>("/api/setup"));
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms, exactly as the status page does: submit
 *  via fetch, read the outcome from the redirect's `?flash=`, show it inline,
 *  and re-poll immediately.
 *
 *  The reason it earns its keep HERE and not only there: the import form holds
 *  a pasted document. A real navigation would throw it away, so a dry run
 *  followed by "now write it" would mean pasting twice. With the page never
 *  navigating, the textarea and both checkboxes survive the round trip and the
 *  second submit is one click.
 *
 *  With JavaScript off the same forms POST, 303 and reload — the dry run's
 *  sentence arrives as the flash, and the document is retyped. Honest, and
 *  still operable. */
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
      redirect: "follow", // 303 → GET /setup?flash=…; the outcome rides res.url
    });
    applyFlash(new URL(res.url).searchParams.get("flash"));
  } catch {
    applyFlash("error: request failed — is ksx studio still running?");
  }
  void poll();
}

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`). */
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
  // One island: the whole screen, seeded from the same SetupPayload JSON that
  // /api/setup serves. Order matters (docs/FORMA-DOGFOOD.md #5): the signals
  // MUST hold the server's values BEFORE SetupIsland() builds the tree.
  SetupIsland: (el) => {
    const seed = embeddedPayload<SetupPayload>();
    if (seed) {
      applySetup(seed);
      applyFlash(seed.flash);
      if (seed.flash && window.location.search !== "") {
        // The flash arrived via /setup?flash=…; clean the URL so a manual
        // reload does not replay stale feedback.
        window.history.replaceState(null, "", "/setup");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return SetupIsland();
  },
});
