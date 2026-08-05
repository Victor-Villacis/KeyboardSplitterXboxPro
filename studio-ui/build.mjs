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
// Post-build: strip the client JS. The skeleton is pure SSR + meta refresh —
// shipping the bundle would let the client runtime re-mount with the signal
// DEFAULTS and clobber the server-rendered data between refreshes. The route
// keeps its `ir` entry (set from the js name before we empty it), which is
// all the Rust side needs. Client JS returns when Studio grows islands.
// ---------------------------------------------------------------------------
const manifestPath = join(outputDir, "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
for (const route of Object.values(manifest.routes)) route.js = [];
for (const [logical, hashed] of Object.entries(manifest.assets)) {
  if (logical.endsWith(".js")) {
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
console.log(`OK: ${irName} is FMIR v2; client JS stripped (SSR-only skeleton)`);
