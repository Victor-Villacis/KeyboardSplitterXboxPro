// Does the SSR FIRST PAINT match what the client shows after hydration?
//
// WHY THIS EXISTS. ksx Studio emits the same data twice — slots for the
// server-rendered paint, the `__ksx-payload` block for the client — and the
// two are produced by two different derivations in two different languages:
// `render_map.rs`/`render.rs` on the server, `MapIsland.ts`/`StatusIsland.ts`
// on the client. Nothing in either toolchain checks that they agree. When they
// disagree the page paints one thing and replaces it milliseconds later: a
// flash, in production, that no unit test on either side can see.
//
// It is not hypothetical. This suite's first run (2026-08-06) found the mapper
// telling a user with a running session to "Use the Pause button above" after
// hydration where the server had said 'Use "Pause emulation & map" above' —
// the second is the button's real label, so the client was both stale AND
// naming a control that does not exist. Only the RUNNING state shows it, which
// is why the fixture learned to be running (`KSX_FIXTURE_SESSION`).
//
// It is also the standing guard on ledger #4: show slots carry real SSR
// defaults now, so a compiler bump that changed which branch renders
// server-side would show up here as a whole panel appearing or disappearing.
//
// Run: cargo build -p ksx-studio --example macro_fixture && npm test

import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { chromium } from "playwright";

/** Our own port — never 4460 (a real `ksx studio`) and never the macro
 *  suite's. The two suites must not run at the same time either: they share
 *  one `macro_fixture.exe`, and Windows will not relink a running binary. See
 *  `--test-concurrency=1` in package.json. */
const PORT = Number(process.env.KSX_PWTEST_PARITY_PORT ?? 4477);

/** ONE PORT PER SESSION STATE, and here is why that matters.
 *
 *  These suites used to share `PORT`. `after()` calls `server.kill()`, which
 *  returns as soon as the signal is sent — the listening socket is still
 *  bound when the next suite's `before()` spawns its fixture a moment later.
 *  That fixture prints "failed to bind 127.0.0.1:4477" and exits… into
 *  `stdio: "ignore"`, so nobody ever saw it. The `before()` hook then spun
 *  for its full 60 s and failed with "fixture never answered", which names
 *  the symptom and hides the cause.
 *
 *  The effect was not flakiness, it was a DETERMINISTIC HOLE: `idle` (first)
 *  passed, `running` (second) always failed, and `down` (third) passed again
 *  because 60 s had elapsed by then and the port was free. So the state this
 *  file's own header calls out — "Only the RUNNING state shows it, which is
 *  why the fixture learned to be running" — was the one state never actually
 *  checked, on every run since it was added.
 *
 *  A port per state removes the reuse entirely; nothing has to wait on
 *  Windows releasing a socket. */
const portFor = (session) => PORT + 1 + SESSIONS.indexOf(session);
const baseFor = (session) => `http://127.0.0.1:${portFor(session)}`;

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.join(repoRoot, "target");
const exe = path.join(
  targetDir,
  "debug",
  "examples",
  process.platform === "win32" ? "macro_fixture.exe" : "macro_fixture",
);

/** The three routes that render server-side. */
const ROUTES = ["/", "/map", "/map?slot=1"];

/** The session states the show pairs actually move between. `modal open` is
 *  deliberately absent: every modal show (`show:modalOpen` and its three inner
 *  states) is injected `false` unconditionally, so the modal CANNOT be part of
 *  a first paint and has no SSR side to disagree with. */
const SESSIONS = ["idle", "running", "down"];

let browser;

/** The island subtree, normalized for the differences that are BY DESIGN.
 *  Each removal is a decision, not a convenience — anything left over is a
 *  real disagreement between the server's paint and the client's. */
const SNAPSHOT = `(() => {
  const root = document.querySelector("[data-forma-island]");
  if (!root) return "NO ISLAND ROOT";
  const clone = root.cloneNode(true);
  // 1. The runtime's own lifecycle marker: "pending" before hydration,
  //    "active" after. It IS the thing that changed; it is not content.
  clone.removeAttribute("data-forma-status");
  // 2. Native island props (ledger #8): an HTML-escaped duplicate of the
  //    markup, emitted only server-side. Not content either.
  clone.removeAttribute("data-forma-props");
  clone.querySelectorAll("[data-forma-props]").forEach((el) => el.removeAttribute("data-forma-props"));
  // 3. The client's own "JavaScript is live" marker class — the whole point of
  //    which is to differ.
  clone.classList.remove("js");
  clone.querySelectorAll(".js").forEach((el) => el.classList.remove("js"));
  let html = clone.outerHTML;
  // 4. forma-ir's U+200B placeholders, which hold the position of an empty
  //    dynamic text slot server-side.
  html = html.replace(/\\u200B/g, "");
  // 5. SSR reconcile markers (<!--f:...-->), the walker's own bookkeeping.
  html = html.replace(/<!--[^>]*?-->/g, "");
  // 6. An EMPTY value attribute the client writes into the form controls it
  //    owns. Empty only — a value the two sides disagree ABOUT still fails.
  html = html.replace(/ value=""/g, "");
  return html;
})()`;

/** The page as the server painted it: the app bundle is blocked, so the
 *  island never hydrates. (Blocking the request beats `javaScriptEnabled:
 *  false`, which would also disable `evaluate`.) */
async function ssrDom(url) {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1400 } });
  await ctx.route("**/_assets/*.js", (route) => route.abort());
  try {
    const page = await ctx.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded" });
    return await page.evaluate(SNAPSHOT);
  } finally {
    await ctx.close();
  }
}

/** The same page after the island has adopted it and seeded from the payload. */
async function hydratedDom(url) {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1400 } });
  try {
    const page = await ctx.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      () => document.querySelector("[data-forma-island]")?.dataset.formaStatus === "active",
      null,
      { timeout: 20_000 },
    );
    // One frame past adoption, before the 2 s poller could rewrite anything:
    // the flash this suite hunts happens in that window.
    await page.waitForTimeout(300);
    return await page.evaluate(SNAPSHOT);
  } finally {
    await ctx.close();
  }
}

/** Point at the first difference in words rather than in 190 000 characters. */
function firstDifference(a, b) {
  const at = a.split(/(?=<)/);
  const bt = b.split(/(?=<)/);
  for (let i = 0; i < Math.max(at.length, bt.length); i++) {
    if (at[i] !== bt[i]) {
      return `node #${i} of ${at.length}\n  SSR      : ${JSON.stringify((at[i] ?? "<missing>").slice(0, 400))}\n  HYDRATED : ${JSON.stringify((bt[i] ?? "<missing>").slice(0, 400))}`;
    }
  }
  return "(lengths differ with no differing node — check trailing content)";
}

before(async () => {
  // Guard every port a suite will actually bind, not the base one that no
  // suite uses any more — a squatter check that watches the wrong door is
  // how the bind failure below became invisible in the first place.
  for (const session of SESSIONS) {
    const url = baseFor(session);
    const squatter = await fetch(`${url}/api/map`).then(
      () => true,
      () => false,
    );
    assert.equal(
      squatter,
      false,
      `something is already listening on ${url} (session "${session}") — stop it first`,
    );
  }
  const built = spawnSync(
    "cargo",
    ["build", "--quiet", "-p", "ksx-studio", "--example", "macro_fixture"],
    { cwd: repoRoot, stdio: "inherit", shell: process.platform === "win32" },
  );
  assert.equal(built.status, 0, "could not build the ksx-studio macro fixture");
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
});

for (const session of SESSIONS) {
  describe(`SSR and hydration agree — session: ${session}`, () => {
    let server;

    const base = baseFor(session);

    before(async () => {
      // stderr is CAPTURED, not ignored: the one thing this fixture says when
      // it cannot start is the one thing worth knowing, and swallowing it is
      // what turned a bind collision into a 60 s mystery.
      let stderr = "";
      server = spawn(exe, [String(portFor(session))], {
        cwd: repoRoot,
        stdio: ["ignore", "ignore", "pipe"],
        env: { ...process.env, KSX_FIXTURE_SESSION: session },
      });
      server.stderr?.on("data", (b) => (stderr += b.toString()));
      let exited = null;
      server.on("exit", (code) => (exited = code));

      const until = Date.now() + 60_000;
      for (;;) {
        const up = await fetch(`${base}/api/map`).then(
          (r) => r.ok,
          () => false,
        );
        if (up) break;
        assert.equal(
          exited,
          null,
          `the fixture for session "${session}" exited with code ${exited} instead of ` +
            `serving ${base}:\n${stderr.trim() || "(it said nothing)"}`,
        );
        assert.ok(
          Date.now() < until,
          `fixture never answered on ${base}\n${stderr.trim() || "(it said nothing)"}`,
        );
        await new Promise((r) => setTimeout(r, 200));
      }
    });

    after(() => {
      server?.kill();
    });

    for (const route of ROUTES) {
      test(`${route} paints what the client renders`, async () => {
        const url = base + route;
        const ssr = await ssrDom(url);
        const hydrated = await hydratedDom(url);
        assert.notEqual(ssr, "NO ISLAND ROOT", `${route} rendered no island root`);
        assert.equal(
          ssr,
          hydrated,
          `${route} (session ${session}) flashes: the server paints one thing and the ` +
            `client replaces it on adoption. Whichever side is wrong, the two ` +
            `derivations have drifted — server: render.rs / render_map.rs, client: ` +
            `StatusIsland.ts / MapIsland.ts.\n${firstDifference(ssr, hydrated)}`,
        );
      });
    }
  });
}
