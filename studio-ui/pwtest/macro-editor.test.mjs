// The macro editor's duration controls, in a real browser.
//
// WHY THIS LEVEL: everything that went wrong here lives in the client island
// and nowhere else — which step the editor points at, the unit that step was
// authored in, and the 2 s poll that re-seeds the draft from the file. Rust
// cannot see any of it (the server only serves the preset), and neither can a
// render test. So these drive the real page against a real ksx Studio, wired
// to a fixed preset by `cargo run -p ksx-studio --example macro_fixture`.
//
// THE BUG THEY PIN (Victor, v12): "the duration unit in ms, when I try to
// change it it just resets — I select the step and the timer and then go hover
// over the duration and it resets." The chain was: selecting a step was not an
// EDIT, so the poll's re-seed treated the draft as untouched and cleared the
// selection two seconds later; with no step selected the unit control had
// nothing to describe, `macroSetUnit` found nothing to set, and the sync that
// follows every poll wrote "ms" back over the author's pick — which arrived
// just as the pointer got there.
//
// Run: cargo build -p ksx-studio --example macro_fixture && npm test

import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { chromium } from "playwright";

/** OUR port, never the 4460 a real `ksx studio` sits on. */
const PORT = Number(process.env.KSX_PWTEST_PORT ?? 4474);
const BASE = `http://127.0.0.1:${PORT}`;
/** map.ts's POLL_MS is 2000; anything above it has crossed at least one poll. */
const PAST_ONE_POLL = 2600;

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

let server;
let browser;

async function waitForServer(deadlineMs = 120_000) {
  const until = Date.now() + deadlineMs;
  for (;;) {
    try {
      const res = await fetch(`${BASE}/api/map`);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    if (Date.now() > until) throw new Error(`fixture server never answered on ${BASE}`);
    await new Promise((r) => setTimeout(r, 250));
  }
}

before(async () => {
  // Refuse to test against somebody else's server. A stale fixture — or a real
  // `ksx studio` — answering here would make every assertion below a story
  // about the wrong build.
  const squatter = await fetch(`${BASE}/api/map`).then(
    () => true,
    () => false,
  );
  assert.equal(squatter, false, `something is already listening on ${BASE} — stop it first`);

  // Build, then run the BINARY — not `cargo run`. A cargo wrapper leaves the
  // real server orphaned when the suite tears down (killing cargo does not
  // kill its child on Windows), and an orphan holding 4474 is then what the
  // NEXT run silently tests against.
  const built = spawnSync(
    "cargo",
    ["build", "--quiet", "-p", "ksx-studio", "--example", "macro_fixture"],
    { cwd: repoRoot, stdio: "inherit", shell: process.platform === "win32" },
  );
  assert.equal(built.status, 0, "could not build the ksx-studio macro fixture");
  const exe = path.join(
    repoRoot,
    "target",
    "debug",
    "examples",
    process.platform === "win32" ? "macro_fixture.exe" : "macro_fixture",
  );
  server = spawn(exe, [String(PORT)], { cwd: repoRoot, stdio: "ignore" });
  server.on("exit", (code) => {
    if (code !== null && code !== 0) {
      throw new Error(`the fixture server exited with ${code} — is ${BASE} already in use?`);
    }
  });
  await waitForServer();
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
  server?.kill();
});

/** A page with the macro card OPEN — it ships collapsed (`<details>`), and
 *  everything under test is inside it. */
async function openEditor() {
  const page = await browser.newPage({ viewport: { width: 1600, height: 1200 } });
  await page.goto(`${BASE}/map`, { waitUntil: "domcontentloaded" });
  // The island is live once map.ts marks it; before that the editor is the
  // no-JS surface and none of this exists.
  await page.waitForFunction(() => document.querySelector(".studio.mapper")?.classList.contains("js"));
  await page.locator(".macrocard > summary").click();
  await page.waitForSelector(".macedit .macunit", { state: "visible" });
  return page;
}

/** Everything an assertion here cares about, read off the live DOM. */
function editorState(page) {
  return page.evaluate(() => ({
    unit: document.querySelector(".macunit")?.value ?? null,
    duration: document.querySelector(".macdurin")?.value ?? null,
    selectedRow: [...document.querySelectorAll(".macrow")].findIndex((r) =>
      r.classList.contains("sel"),
    ),
    stepLine: document.querySelector(".macsteplbl")?.textContent ?? "",
    activeClass: document.activeElement?.className ?? "",
  }));
}

const settle = (page) => page.waitForTimeout(PAST_ONE_POLL);

describe("the macro editor's duration controls", () => {
  test("the selected step survives the 2 s poll", async () => {
    // The root cause: the poll re-seeded every CLEAN draft, and the re-seed
    // dropped the selection — so the duration editor quietly let go of the
    // step the author had just picked, with nothing on screen to say so.
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|0"]').click();
      assert.equal((await editorState(page)).selectedRow, 0);

      await settle(page);

      const after = await editorState(page);
      assert.equal(after.selectedRow, 0, "the poll let go of the selected step");
      assert.match(after.stepLine, /^step 1 of 3/);
    } finally {
      await page.close();
    }
  });

  test("the authored unit survives a poll, then a hover — Victor's repro", async () => {
    const page = await openEditor();
    try {
      // Step 1 is authored in ms in the preset.
      await page.locator('[data-macact="sel|0"]').click();
      assert.equal((await editorState(page)).unit, "ms");

      // The pause that used to be fatal: a poll lands between picking the step
      // and reaching the unit control.
      await settle(page);

      await page.locator(".macunit").selectOption("frames");
      const picked = await editorState(page);
      assert.equal(picked.unit, "frames", "the unit control snapped back on its own");
      // CONVERTED, not reinterpreted: 50 ms is 3 frames, not 50 of them.
      assert.equal(picked.duration, "3");
      assert.match(picked.stepLine, /3 fr/);

      // …and now the pointer arrives, which is when Victor saw it reset.
      await page.locator(".macedit").hover();
      await page.locator(".macrow").first().hover();
      await page.locator("[data-fn]").first().hover();
      assert.equal((await editorState(page)).unit, "frames", "a hover reset the unit");

      // …and a poll after that.
      await settle(page);
      const later = await editorState(page);
      assert.equal(later.unit, "frames", "the poll reset the unit");
      assert.equal(later.duration, "3");
    } finally {
      await page.close();
    }
  });

  test("a hover never steals focus or rewrites a duration being typed", async () => {
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|1"]').click(); // step 2, authored in frames
      await page.locator(".macunit").selectOption("ms");

      const box = page.locator(".macdurin");
      await box.click();
      await box.fill("120");
      assert.equal((await editorState(page)).activeClass, "macdurin");

      // Hover the zones and the legend — both re-derive their lists, which is
      // what a rebuild under the caret would destroy.
      await page.locator("[data-fn]").first().hover();
      await page.locator(".macrow").first().hover();
      await settle(page);

      const held = await editorState(page);
      assert.equal(held.activeClass, "macdurin", "focus was taken mid-edit");
      assert.equal(held.duration, "120", "the box was rewritten mid-edit");
      assert.equal(held.unit, "ms");
    } finally {
      await page.close();
    }
  });

  test("the authored unit survives Save and a reload", async () => {
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|2"]').click(); // step 3, authored in ms
      await page.locator(".macunit").selectOption("frames");
      assert.equal((await editorState(page)).unit, "frames");

      await page.locator('[data-act="macro-save"]').click();
      await page.waitForFunction(() =>
        (document.querySelector(".macdirty")?.textContent ?? "").startsWith("saved"),
      );
      assert.equal((await editorState(page)).unit, "frames", "the save round trip lost the unit");

      await page.reload({ waitUntil: "domcontentloaded" });
      await page.waitForFunction(() =>
        document.querySelector(".studio.mapper")?.classList.contains("js"),
      );
      await page.locator(".macrocard > summary").click();
      await page.locator('[data-macact="sel|2"]').click();
      assert.equal(
        (await editorState(page)).unit,
        "frames",
        "the reloaded preset came back in ms",
      );
    } finally {
      await page.close();
    }
  });
});
