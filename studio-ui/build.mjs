// ksx Studio UI build — TS → FMIR + hashed assets into crates/ksx-studio/assets/.
//
// Run with plain `node build.mjs` (no tsx needed — see
// docs/research/forma-spike-1-fmir-compat.md). Plain-CSS entries only: this
// page needs no tailwind. (The @getforma/build 0.1.8 Windows bug — `npx`
// spawned via execFileSync without shell:true, ENOENT — was fixed in 0.1.9,
// so a `tailwind: true` cssEntry would now work if ever wanted.)

import { build } from "@getforma/build";
import { readFileSync, writeFileSync, rmSync } from "fs";
import { join } from "path";

const outputDir = "../crates/ksx-studio/assets";

await build({
  entryPoints: [{ entry: "src/status.ts", outfile: "status.js" }],
  cssEntries: [{ input: "src/studio.css", outfile: "studio.css" }],
  routes: { "/": { js: ["status"], css: ["studio"] } },
  outputDir,
  ssr: true,
  ssrEntryPoints: { status: "src/status.ts" },
});

// ---------------------------------------------------------------------------
// The client JS SHIPS again (v4). The v3 build stripped it here, because the
// only hydration path then was mount(), which re-mounts with signal DEFAULTS
// and clobbers the server-rendered data (dogfood ledger #5). v4 uses the
// islands protocol instead: src/status.ts registers the island via
// activateIslands, and the signals are seeded from the server's props
// (__forma_islands script block) BEFORE adoption — SSR values survive, then
// the /api/status poller keeps them live. The route keeps its js entry and
// the manifest keeps the hashed bundle.
//
// What still gets removed: the compiler's island BYPRODUCTS. @getforma/build
// auto-generates `status.islands.js`, a registry that (a) maps the island to
// the page ROOT component — the exact clobber pattern ledger #5 bans — and
// (b) imports `../src/...` relative to the output dir, which does not even
// resolve here. Our entry does its own activateIslands; the byproducts must
// not be embedded by rust-embed or linger in the manifest.
// ---------------------------------------------------------------------------
const manifestPath = join(outputDir, "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
for (const [logical, hashed] of Object.entries(manifest.assets)) {
  if (logical.startsWith("status.islands.")) {
    for (const f of [hashed, `${hashed}.br`, `${hashed}.gz`]) {
      rmSync(join(outputDir, f), { force: true });
    }
    delete manifest.assets[logical];
  }
}
rmSync(join(outputDir, "status.islands.json"), { force: true });
writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");

// ---------------------------------------------------------------------------
// FMIR version guard (docs/research/forma-spike-1-fmir-compat.md "cheap
// insurance"): forma-server 0.1.4 renders FMIR v2 only. Refuse to emit
// anything a compiler bump silently made incompatible.
// ---------------------------------------------------------------------------
const irName = manifest.routes["/"].ir;
if (!irName) throw new Error("manifest route '/' lost its .ir entry");
const ir = readFileSync(join(outputDir, irName));
const magic = ir.subarray(0, 4).toString("latin1");
const version = ir.readUInt16LE(4);
if (magic !== "FMIR" || version !== 2) {
  throw new Error(
    `IR guard failed: ${irName} is ${magic} v${version}, expected FMIR v2 ` +
      "(forma-server 0.1.4 contract)",
  );
}
if (!manifest.routes["/"].js?.length) {
  throw new Error(
    "manifest route '/' has no client js — the island runtime cannot load",
  );
}
console.log(`OK: ${irName} is FMIR v2; island client bundle kept (v4)`);
