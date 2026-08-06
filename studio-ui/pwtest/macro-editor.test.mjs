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
const PORT = Number(process.env.KSX_PWTEST_PORT ?? 4476);
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
    // FIX 1: the plain-language readout on every row — what this step holds.
    holds: [...document.querySelectorAll(".macrowbar .machold")].map((h) => h.textContent),
    holdClasses: [...document.querySelectorAll(".macrowbar .machold")].map(
      (h) => h.className,
    ),
    // v16, THE LEDGER: the pair each diagonal on that row is STORED as.
    expands: [...document.querySelectorAll(".macrowbar .macexp")].map((e) => e.textContent),
    expandClasses: [...document.querySelectorAll(".macrowbar .macexp")].map(
      (e) => e.className,
    ),
    toasts: [...document.querySelectorAll(".toasts .tmsg")].map((t) => t.textContent),
    undoable: document.querySelectorAll(".toasts [data-undo]:not(.off)").length,
    toml: document.querySelector(".mactoml")?.textContent ?? "",
    // FIX 2: the duration field's own warn class, and Save's question.
    durClass: document.querySelector(".macdurin")?.className ?? "",
    confirmClass: document.querySelector(".macconfirm")?.className ?? "",
    confirmLine: document.querySelector(".macconfirmline")?.textContent ?? "",
    shortRows: document.querySelectorAll(".macrow.short").length,
    dirty: document.querySelector(".macdirty")?.textContent ?? "",
  }));
}

/** The same page, pointed at the fixture's `written-by-hand` macro — the steps
 *  NOBODY MADE THROUGH THIS PAGE. Switching macros is a real route, so this
 *  goes through the tab the way a reader would. */
async function openHandwritten() {
  const page = await openEditor();
  await page.locator('[data-macro="written-by-hand"]').click();
  await page.waitForFunction(
    () => (document.querySelector(".machead")?.textContent ?? "").startsWith("written-by-hand"),
  );
  return page;
}

/** One cell's class + mark + title, by its `data-cell` payload. */
function cellState(page, cell) {
  return page.evaluate((sel) => {
    const el = document.querySelector(`[data-cell="${sel}"]`);
    return el === null
      ? null
      : { cls: el.className, mark: el.textContent, title: el.getAttribute("title") };
  }, cell);
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

// ── FIX 1: a row can hold several controls, and it SAYS so ─────────────────
// The evening this pins: Victor's "hadouken" had rows ↓ then → then X and no
// diagonal at all, because a diagonal is not an input you bind — it IS
// down+forward held together, i.e. ONE ROW HOLDING TWO CONTROLS. The piano
// roll never taught that, and two lit cells twelve columns apart never will.
// The readout has to be LIVE: it is only a teacher if it changes under the
// finger that is ticking the cells.

describe("what a step holds, in words", () => {
  test("the readout updates as cells are toggled", async () => {
    const page = await openEditor();
    try {
      // The fixture's step 1 holds one direction, and says so.
      const start = await editorState(page);
      assert.equal(start.holds[0], "D-pad ↓");
      assert.equal(start.holdClasses[0], "machold");

      // Tick a SECOND control into the SAME row. It is a diagonal now, so the
      // row reads as the ONE control a player means by it — and spells out the
      // two names the file will carry, so nothing is hidden.
      await page.locator('[data-cell="0|dpad.right"]').click();
      const chord = await editorState(page);
      assert.equal(chord.holds[0], "D-pad ↘", "the pair did not read as the diagonal");
      assert.equal(
        chord.holdClasses[0],
        "machold",
        "a diagonal is ONE presented control, not two",
      );
      assert.equal(chord.expands[0], "↘ = dpad.down + dpad.right");

      // Untick both and the row reads as what it now is: a neutral gap, not a
      // row somebody forgot to fill in.
      await page.locator('[data-cell="0|dpad.right"]').click();
      await page.locator('[data-cell="0|dpad.down"]').click();
      const empty = await editorState(page);
      assert.equal(empty.holds[0], "(nothing — neutral gap)");
      assert.equal(empty.holdClasses[0], "machold none");

      // And it survives a poll, like every other draft edit.
      await settle(page);
      assert.equal((await editorState(page)).holds[0], "(nothing — neutral gap)");
    } finally {
      await page.close();
    }
  });

  test("a common motion inserts the chord step nobody thinks to write", async () => {
    const page = await openEditor();
    try {
      const before = (await editorState(page)).holds.length;
      await page.locator('[data-macmotion="qcf"]').click();
      const after = await editorState(page);
      assert.equal(after.holds.length, before + 3, "a quarter-circle is three steps");

      // The three appended steps, in order — the middle one is the diagonal.
      const added = after.holds.slice(before);
      assert.deepEqual(added, ["D-pad ↓", "D-pad ↘", "D-pad →"]);
      assert.equal(after.holdClasses[before + 1], "machold");
      assert.equal(after.expands[before + 1], "↘ = dpad.down + dpad.right");

      // Generated ABOVE the sampling floor: a helper that seeded steps the
      // sampler cannot see would teach the exact mistake it exists to prevent.
      assert.equal(after.shortRows, 0, "the generated steps are below the floor");
      assert.match(after.dirty, /unsaved/);
    } finally {
      await page.close();
    }
  });
});

// ── v16: DIAGONALS AS PRESENTATION ─────────────────────────────────────────
// Victor: "if down and right together equals diagonal, the user does not care —
// we can present the diagonal in the piano and the user can select it, and
// behind the scenes we do down and right, so it's seamless."
//
// Everything below is that promise, in both directions. The STORED model is
// unchanged — a step still holds a set of ordinary bindings — so the two things
// that must be true are: picking ↘ writes the pair, and a pair NOBODY made
// through this page reads back as ↘.

describe("diagonals are a lens over the stored pair", () => {
  test("picking ↘ writes down + right, and says so", async () => {
    const page = await openEditor();
    try {
      // Step 1 holds `dpad.down` only. The ↘ column is a thing you can point
      // at — no mapper in the field offers one.
      const before = await cellState(page, "0|diag:dpad:dr");
      assert.ok(before, "the D-pad ↘ column does not exist");
      assert.doesNotMatch(before.cls, /\bon\b/);
      assert.match(before.title, /does not hold D-pad ↘ \(down-right\)/);

      await page.locator('[data-cell="0|diag:dpad:dr"]').click();

      // THE PAIR IS WHAT IS STORED. Not a new binding shape, not a new verb —
      // the same two ordinary names a hand-edited file would carry, which is
      // why the engine and every old preset are untouched.
      const after = await editorState(page);
      assert.equal(after.holds[0], "D-pad ↘");
      assert.equal(after.expands[0], "↘ = dpad.down + dpad.right");
      assert.match(after.toml, /hold = \["dpad\.down", "dpad\.right"\]/);

      // The cell is lit, and the two cardinals it is made of show as halves —
      // the lens never hides the storage it exists to explain.
      assert.match((await cellState(page, "0|diag:dpad:dr")).cls, /\bon\b/);
      assert.equal((await cellState(page, "0|dpad.down")).cls.includes("part"), true);
      assert.equal((await cellState(page, "0|dpad.right")).mark, "·");

      // It REPORTS, with undo — the one click on this grid whose effect is not
      // literally the cell you hit.
      assert.ok(
        after.toasts.some((t) => /ksx wrote dpad\.down \+ dpad\.right/.test(t)),
        `no toast named what was written: ${JSON.stringify(after.toasts)}`,
      );
      assert.ok(after.undoable >= 1, "the diagonal pick offered no undo");
      await page.locator(".toasts [data-undo]").first().click();
      await page.waitForFunction(
        () => (document.querySelector(".macrowbar .machold")?.textContent ?? "") === "D-pad ↓",
      );
      assert.equal((await editorState(page)).holds[0], "D-pad ↓", "undo did not put it back");

      // Ticking it again and then UNticking it removes exactly the two.
      await page.locator('[data-cell="0|diag:dpad:dr"]').click();
      await page.locator('[data-cell="0|diag:dpad:dr"]').click();
      const cleared = await editorState(page);
      assert.equal(cleared.holds[0], "(nothing — neutral gap)");
      assert.match(cleared.toml, /hold = \[\]/);
    } finally {
      await page.close();
    }
  });

  test("a hand-written pair reads back as ↘", async () => {
    // The round trip that matters: these steps were never made through this
    // page. Somebody typed them into the preset, or imported them, and the
    // grid still has to show what they ARE.
    const page = await openHandwritten();
    try {
      const state = await editorState(page);
      assert.equal(state.holds[0], "D-pad ↘", "the canonical pair did not fold");
      assert.equal(state.expands[0], "↘ = dpad.down + dpad.right");
      assert.match((await cellState(page, "0|diag:dpad:dr")).cls, /\bon\b/);

      // A PARTIAL deflection is still the diagonal — labelled, never rewritten.
      assert.equal(state.holds[1], "LS ↘");
      assert.equal(state.expands[1], "↘ = ly.-16384 + lx.max");
      const inexact = await cellState(page, "1|diag:ls:dr");
      assert.match(inexact.cls, /\bapprox\b/, "an inexact diagonal is not labelled");
      assert.match(inexact.title, /not at full deflection/);
      assert.match(state.toml, /ly\.-16384/, "the exact value was rewritten");

      // CONTRADICTORY — `down + forward + up`. Never folded, never guessed:
      // which diagonal would it be, and what the pad publishes depends on the
      // slot's socd policy, which this page cannot see.
      assert.equal(state.holds[3], "D-pad ↓ + D-pad → + D-pad ↑");
      assert.equal(state.expandClasses[3], "macexp off");
      assert.doesNotMatch((await cellState(page, "3|diag:dpad:dr")).cls, /\bon\b/);
      assert.doesNotMatch((await cellState(page, "3|diag:dpad:ur")).cls, /\bon\b/);

      // The hat+stick double-binding EVERY in-box template writes: one
      // diagonal, naming both mechanisms, lit on both groups.
      assert.equal(state.holds[4], "D-pad + LS ↘");
      assert.match((await cellState(page, "4|diag:dpad:dr")).cls, /\bon\b/);
      assert.match((await cellState(page, "4|diag:ls:dr")).cls, /\bon\b/);

      // …and the file is untouched by any of this looking at it.
      assert.match(state.dirty, /^$|saved/, `reading a macro marked it dirty: ${state.dirty}`);
    } finally {
      await page.close();
    }
  });

  test("a diagonal + button step still shows both", async () => {
    // The single most common macro step in existence — the attack that ends a
    // motion. Exact-set matching on the whole step would have failed it, which
    // is what settles the whole recognition rule.
    const page = await openHandwritten();
    try {
      const state = await editorState(page);
      assert.equal(state.holds[2], "D-pad ↘ + A");
      assert.equal(
        state.holdClasses[2],
        "machold both",
        "a diagonal AND a button is genuinely two presented controls",
      );
      assert.equal(state.expands[2], "↘ = dpad.down + dpad.right");
      assert.match((await cellState(page, "2|diag:dpad:dr")).cls, /\bon\b/);
      assert.match((await cellState(page, "2|A")).cls, /\bon\b/);

      // Adding the attack to a bare diagonal does the same thing live — the
      // button is a passenger, and the diagonal survives it.
      await page.locator('[data-cell="0|B"]').click();
      const after = await editorState(page);
      assert.equal(after.holds[0], "D-pad ↘ + B");
      assert.equal(after.expands[0], "↘ = dpad.down + dpad.right");
    } finally {
      await page.close();
    }
  });

  test("all four diagonals are picks, and the up ones deflect UP", async () => {
    // ⚠ THE SIGN. "Up" on a stick is `ly.max`, not `ly.min` — XInput's positive
    // Y is up. A mirrored sign gives an ↖ that looks perfect in every reader on
    // this page and does nothing in the game, so the assertion is on the TOML
    // the file will carry, name by name.
    const page = await openEditor();
    try {
      const want = {
        "diag:dpad:ul": ["dpad.up", "dpad.left"],
        "diag:dpad:ur": ["dpad.up", "dpad.right"],
        "diag:dpad:dl": ["dpad.down", "dpad.left"],
        "diag:dpad:dr": ["dpad.down", "dpad.right"],
        "diag:ls:ul": ["ly.max", "lx.min"],
        "diag:ls:ur": ["ly.max", "lx.max"],
        "diag:ls:dl": ["ly.min", "lx.min"],
        "diag:ls:dr": ["ly.min", "lx.max"],
        "diag:rs:ul": ["ry.max", "rx.min"],
        "diag:rs:ur": ["ry.max", "rx.max"],
        "diag:rs:dl": ["ry.min", "rx.min"],
        "diag:rs:dr": ["ry.min", "rx.max"],
      };
      const glyph = { ul: "↖", ur: "↗", dl: "↙", dr: "↘" };
      const group = { dpad: "D-pad", ls: "LS", rs: "RS" };
      // Start from a clean row, so each pick is the only thing in it.
      await page.locator('[data-cell="0|dpad.down"]').click();
      assert.equal((await editorState(page)).holds[0], "(nothing — neutral gap)");

      for (const [token, pair] of Object.entries(want)) {
        const [, mech, d] = token.split(":");
        await page.locator(`[data-cell="0|${token}"]`).click();
        const on = await editorState(page);
        assert.equal(
          on.holds[0],
          `${group[mech]} ${glyph[d]}`,
          `${token} did not read back as itself`,
        );
        assert.equal(on.expands[0], `${glyph[d]} = ${pair.join(" + ")}`);
        assert.ok(
          on.toml.includes(`hold = ["${pair[0]}", "${pair[1]}"]`),
          `${token} stored the wrong pair — the file says ${on.toml.split("\n")[2]}`,
        );
        // Picking the NEXT diagonal on the same mechanism replaces this one
        // rather than contradicting it; a different mechanism would not, which
        // is why each is unticked before moving on.
        await page.locator(`[data-cell="0|${token}"]`).click();
        assert.equal((await editorState(page)).holds[0], "(nothing — neutral gap)");
      }
    } finally {
      await page.close();
    }
  });

  test("a 360 walks all eight positions, four of them diagonals", async () => {
    // THE MOTION THE FEATURE IS FOR. A spinning piledriver needs ↘ ↙ ↖ ↗ — if
    // only down-forward were first class, the helper could not write it and the
    // grid could not show it. Every inserted step displaying as its own
    // diagonal is the proof that recognition and expansion agree.
    const page = await openEditor();
    try {
      const before = (await editorState(page)).holds.length;
      await page.locator('[data-macmotion="spdf"]').click();
      const after = await editorState(page);
      assert.equal(after.holds.length, before + 8, "a 360 is eight steps");
      assert.deepEqual(after.holds.slice(before), [
        "D-pad →",
        "D-pad ↘",
        "D-pad ↓",
        "D-pad ↙",
        "D-pad ←",
        "D-pad ↖",
        "D-pad ↑",
        "D-pad ↗",
      ]);
      // Each diagonal step spells the pair it stores — including the two UP
      // ones, which are the sign trap.
      assert.equal(after.expands[before + 1], "↘ = dpad.down + dpad.right");
      assert.equal(after.expands[before + 3], "↙ = dpad.down + dpad.left");
      assert.equal(after.expands[before + 5], "↖ = dpad.up + dpad.left");
      assert.equal(after.expands[before + 7], "↗ = dpad.up + dpad.right");
      assert.equal(after.shortRows, 0, "the generated steps are below the floor");

      // The mirrored facing is the same circle the other way round.
      const page2 = await openEditor();
      try {
        const n = (await editorState(page2)).holds.length;
        await page2.locator('[data-macmotion="spdb"]').click();
        assert.deepEqual((await editorState(page2)).holds.slice(n), [
          "D-pad ←",
          "D-pad ↙",
          "D-pad ↓",
          "D-pad ↘",
          "D-pad →",
          "D-pad ↗",
          "D-pad ↑",
          "D-pad ↖",
        ]);
      } finally {
        await page2.close();
      }
    } finally {
      await page.close();
    }
  });

  test("the grid is three rings, and the SSR paint already says so", async () => {
    // The mirror: MapIsland.ts re-derives every list on each poll, so the
    // hydrated grid and the server's first paint have to agree column for
    // column — the same rule the zone tables live under.
    const ssr = await fetch(`${BASE}/map`).then((r) => r.text());
    const page = await openEditor();
    try {
      const live = await page.evaluate(() => ({
        columns: [...document.querySelectorAll(".maccols .maccolid")].map((c) => c.textContent),
        bands: [...document.querySelectorAll(".macgrps .macgrp")].map((g) => g.textContent),
        spans: [...document.querySelectorAll(".macgrps .macgrp")].map((g) => g.className),
      }));
      assert.equal(live.columns.length, 37, "25 zones → 37 columns");
      // Ring order — ↑ ↖ ← ↙ ↓ ↘ → ↗ — three times, which is what makes a
      // motion a SHAPE rather than a blob.
      const ring = ["↑", "↖", "←", "↙", "↓", "↘", "→", "↗"];
      assert.deepEqual(live.columns.slice(12, 20), ring, "the left stick's ring");
      assert.deepEqual(live.columns.slice(20, 28), ring, "the d-pad's ring");
      assert.deepEqual(live.columns.slice(29, 37), ring, "the right stick's ring");
      assert.deepEqual(live.bands, [
        "SHOULDERS",
        "FACE",
        "SYSTEM",
        "LEFT STICK",
        "D-PAD",
        "RIGHT STICK",
      ]);
      assert.deepEqual(live.spans, [
        "macgrp g4",
        "macgrp g4",
        "macgrp g3",
        "macgrp g9",
        "macgrp g8",
        "macgrp g9",
      ]);
      // The no-JS page carries the same columns and the ring line that
      // explains them — it cannot tick a cell, but it can read what a pick
      // would write and hand-edit the TOML block below.
      assert.ok(ssr.includes('data-cell="0|diag:dpad:dr"'), "SSR has no diagonal column");
      assert.ok(ssr.includes("↑ ↖ ← ↙ ↓ ↘ → ↗ (numpad 8 7 4 1 2 3 6 9)"), "SSR has no ring line");
      assert.ok(ssr.includes("stores dpad.down + dpad.right on that step"));
    } finally {
      await page.close();
    }
  });
});

// ── FIX 2: a below-floor step cannot be missed ─────────────────────────────
// The advisory existed and read as decoration, so the 1-frame steps went in
// anyway. It is now on the offending FIELD, on the ROW, and between the click
// and the write — but it never refuses, because a short step is legal.

describe("a step shorter than the sampling floor", () => {
  test("a 1-frame step marks its field and its row, in frames", async () => {
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|0"]').click();
      await page.locator(".macunit").selectOption("frames");
      const box = page.locator(".macdurin");
      await box.click();
      await box.fill("1");
      await box.blur();

      const state = await editorState(page);
      assert.equal(state.duration, "1");
      assert.match(state.durClass, /\bshort\b/, "the duration field is not flagged");
      assert.equal(state.shortRows, 1, "the row is not marked");
      // Answered in the unit it was AUTHORED in — not "16 ms", a number this
      // author never typed.
      assert.match(state.stepLine, /1 frame is shorter than the 2-frame floor/);

      // Back above the floor and every mark goes away.
      await box.fill("3");
      await box.blur();
      const fixed = await editorState(page);
      assert.equal(fixed.durClass, "macdurin");
      assert.equal(fixed.shortRows, 0);
    } finally {
      await page.close();
    }
  });

  // Runs BEFORE the test that saves a short step: the fixture keeps what Save
  // wrote (that is what makes "survives a reload" testable at all), so an
  // "ordinary save" case has to be asserted while the preset is still ordinary.
  test("a macro with nothing short saves on the first click", async () => {
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|0"]').click(); // 50 ms, authored in ms
      const box = page.locator(".macdurin");
      await box.click();
      await box.fill("60");
      await box.blur();
      assert.equal((await editorState(page)).shortRows, 0);

      await page.locator('[data-act="macro-save"]').click();
      await page.waitForFunction(() =>
        (document.querySelector(".macdirty")?.textContent ?? "").startsWith("saved"),
      );
      assert.match(
        (await editorState(page)).confirmClass,
        /\boff\b/,
        "an ordinary save asked a question it had no reason to ask",
      );
    } finally {
      await page.close();
    }
  });

  test("Save asks before it writes one, and never refuses", async () => {
    const page = await openEditor();
    try {
      await page.locator('[data-macact="sel|0"]').click();
      await page.locator(".macunit").selectOption("frames");
      const box = page.locator(".macdurin");
      await box.click();
      await box.fill("1");
      await box.blur();

      // FIRST click: the question, not the write.
      await page.locator('[data-act="macro-save"]').click();
      const asked = await editorState(page);
      assert.doesNotMatch(asked.confirmClass, /\boff\b/, "Save wrote it silently");
      assert.match(asked.confirmLine, /1 step is shorter than ~33 ms \(2 frames at 60 Hz\)/);
      assert.match(asked.confirmLine, /Save anyway\?$/);
      assert.match(asked.dirty, /unsaved/, "the draft was written while being asked about");

      // The question survives the 2 s poll — a dialog that vanishes on its own
      // is the same silent save with extra steps.
      await settle(page);
      assert.doesNotMatch(
        (await editorState(page)).confirmClass,
        /\boff\b/,
        "the poll took the question down",
      );

      // "Not yet" takes it down and leaves the preset alone.
      await page.locator('[data-act="macro-save-cancel"]').click();
      const cancelled = await editorState(page);
      assert.match(cancelled.confirmClass, /\boff\b/);
      assert.match(cancelled.dirty, /unsaved/, "cancelling wrote the macro");

      // Ask again, then answer it: the save goes through exactly as authored.
      await page.locator('[data-act="macro-save"]').click();
      assert.doesNotMatch((await editorState(page)).confirmClass, /\boff\b/);
      await page.locator('[data-act="macro-save-anyway"]').click();
      await page.waitForFunction(() =>
        (document.querySelector(".macdirty")?.textContent ?? "").startsWith("saved"),
      );
      const saved = await editorState(page);
      assert.match(saved.confirmClass, /\boff\b/, "the question outlived the answer");
      // The short step is still short — the save never rewrote it.
      assert.equal(saved.shortRows, 1);
    } finally {
      await page.close();
    }
  });
});
