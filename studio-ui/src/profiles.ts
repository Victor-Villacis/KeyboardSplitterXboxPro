import { activateIslands } from "@getforma/core";
import { fetchJSON } from "@getforma/core/http";

// The compiler's island-first entry pattern (parseEntryPoint): the imported
// `*Page` component NOT in the activateIslands registry is the SSR root.
// ProfilesPage never runs in the browser — esbuild tree-shakes it — but this
// import is what anchors IR emission. Do not remove it.
import { ProfilesPage } from "./ProfilesPage";
import {
  ProfilesIsland,
  applyFlash,
  applyProfiles,
  applyUnreachable,
  type ProfilesPayload,
} from "./ProfilesIsland";

void ProfilesPage; // compile-time anchor only (see above)

/** Same cadence as the other two screens. A profile's brokenness is a fact
 *  about the filesystem, so it changes while you are looking at it — put the
 *  emulator back and the row goes green without a reload. */
const POLL_MS = 2000;

async function poll(): Promise<void> {
  try {
    applyProfiles(await fetchJSON<ProfilesPayload>("/api/profiles"));
  } catch {
    applyUnreachable();
  }
}

/** Fetch-enhance the plain-HTML forms (progressive enhancement: with JS off
 *  they POST + 303 + full reload). Submit via fetch, read the outcome from the
 *  redirect's ?flash= query, flash it inline, and re-read immediately.
 *  Delegated on the island root so rows re-rendered by list reconcile stay
 *  wired. */
function wireForms(root: HTMLElement): void {
  root.addEventListener("submit", (ev) => {
    const form = ev.target as HTMLFormElement | null;
    if (!form || form.method.toLowerCase() !== "post") return;
    ev.preventDefault();
    const confirmation = form.dataset.confirm;
    if (confirmation && !window.confirm(confirmation)) return;
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
      redirect: "follow", // 303 → GET /profiles?flash=…; the outcome rides res.url
    });
    // The outcome rides the REDIRECT. Every route on this page answers a POST
    // with a 303 carrying `?flash=`, so anything else — a 422 the extractor
    // produced, a 403 from the same-origin guard, a 500 — has no flash to
    // read, and `searchParams.get("flash")` would return null, which
    // `applyFlash` renders as nothing at all. A button that silently does
    // nothing is the failure this whole page was written to close; it must
    // not be reachable by any status code, including ones added later.
    const flash = res.redirected
      ? new URL(res.url).searchParams.get("flash")
      : null;
    if (flash !== null) {
      applyFlash(flash);
    } else {
      applyFlash(
        "error: that change could not be accepted. Nothing was changed. Reopen ksx and try again.",
      );
    }
  } catch {
    applyFlash("error: the change could not be sent. Reopen ksx and try again.");
  }
  void poll();
}

/** The SOURCE payload the server embedded (render.rs `PAYLOAD_SCRIPT_ID`) —
 *  deliberately not the island's `props`, which since compiler 0.3.1 carry the
 *  RENDERED SLOT VALUES rather than the shape this page derives from. */
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
  // Ledger #5 order: the signals MUST hold the server's values BEFORE
  // ProfilesIsland() builds the descriptor tree — adoption binds effects that
  // immediately write signal state into the DOM, so seeding after adoption
  // would clobber the SSR text with defaults.
  ProfilesIsland: (el) => {
    const seed = embeddedPayload<ProfilesPayload>();
    if (seed) {
      applyProfiles(seed);
      applyFlash(seed.flash);
      if (seed.flash && window.location.search !== "") {
        // The flash arrived via /profiles?flash=…; clean the URL so a manual
        // reload does not replay stale feedback.
        window.history.replaceState(null, "", "/profiles");
      }
    }
    wireForms(el);
    window.setInterval(() => void poll(), POLL_MS);
    return ProfilesIsland();
  },
});
