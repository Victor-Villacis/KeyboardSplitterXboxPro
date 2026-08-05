// ksx Studio UI build — TS → FMIR + hashed assets into crates/ksx-studio/assets/.
//
// Run with plain `node build.mjs` (no tsx needed — see
// docs/research/forma-spike-1-fmir-compat.md). Plain-CSS entries only: this
// page needs no tailwind. (The @getforma/build 0.1.8 Windows bug — `npx`
// spawned via execFileSync without shell:true, ENOENT — was fixed in 0.1.9,
// so a `tailwind: true` cssEntry would now work if ever wanted.)
//
// v5: TWO routes — "/" (status) and "/map" (the mapper) — plus the vendored
// controller art copied (cleaned) from art/ into the embed.

import { build } from "@getforma/build";
import { readFileSync, writeFileSync, rmSync } from "fs";
import { join } from "path";

const outputDir = "../crates/ksx-studio/assets";

await build({
  entryPoints: [
    { entry: "src/status.ts", outfile: "status.js" },
    { entry: "src/map.ts", outfile: "map.js" },
  ],
  cssEntries: [{ input: "src/studio.css", outfile: "studio.css" }],
  routes: {
    "/": { js: ["status"], css: ["studio"] },
    "/map": { js: ["map"], css: ["studio"] },
  },
  outputDir,
  ssr: true,
  ssrEntryPoints: { status: "src/status.ts", map: "src/map.ts" },
});

// ---------------------------------------------------------------------------
// The client JS SHIPS (v4+): the islands protocol seeds signals from server
// props before adoption (dogfood ledger #5). What still gets removed: the
// compiler's island BYPRODUCTS (`*.islands.js`) — they map each island to the
// page ROOT component (the exact clobber pattern ledger #5 bans) and import
// `../src/...` paths that do not resolve from the output dir. Our entries do
// their own activateIslands; the byproducts must not be embedded by
// rust-embed or linger in the manifest.
// ---------------------------------------------------------------------------
const manifestPath = join(outputDir, "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
for (const [logical, hashed] of Object.entries(manifest.assets)) {
  if (/^(status|map)\.islands\./.test(logical)) {
    for (const f of [hashed, `${hashed}.br`, `${hashed}.gz`]) {
      rmSync(join(outputDir, f), { force: true });
    }
    delete manifest.assets[logical];
  }
}
for (const entry of ["status", "map"]) {
  rmSync(join(outputDir, `${entry}.islands.json`), { force: true });
}
writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");

// ---------------------------------------------------------------------------
// Vendored controller art (Gamepad-Asset-Pack, MIT, by AL2009man — see
// art/README.md): copy the committed sources into the embed under their
// stable serving names, stripped of Inkscape/Sodipodi editor metadata (pure
// byte-diet; geometry untouched). render.rs serves them at
// /_assets/pad-xbox.svg and /_assets/pad-ds4.svg.
// ---------------------------------------------------------------------------
function cleanSvg(source) {
  return readFileSync(join("art", source), "utf8")
    .replace(/<sodipodi:namedview[\s\S]*?\/>/g, "")
    .replace(/<metadata[\s\S]*?<\/metadata>/g, "")
    .replace(/\s+(?:inkscape|sodipodi):[\w-]+="[^"]*"/g, "")
    .replace(/\s+xmlns:(?:inkscape|sodipodi)="[^"]*"/g, "")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/\n{3,}/g, "\n\n");
}
const ART = [
  ["src-xboxseries.svg", "pad-xbox.svg"],
  ["src-ds4.svg", "pad-ds4.svg"],
];
for (const [source, out] of ART) {
  writeFileSync(join(outputDir, out), cleanSvg(source));
}

// ---------------------------------------------------------------------------
// FMIR version guard (docs/research/forma-spike-1-fmir-compat.md "cheap
// insurance"): forma-server 0.1.4 renders FMIR v2 only. Refuse to emit
// anything a compiler bump silently made incompatible — for EVERY route.
// ---------------------------------------------------------------------------
for (const route of ["/", "/map"]) {
  const irName = manifest.routes[route].ir;
  if (!irName) throw new Error(`manifest route '${route}' lost its .ir entry`);
  const ir = readFileSync(join(outputDir, irName));
  const magic = ir.subarray(0, 4).toString("latin1");
  const version = ir.readUInt16LE(4);
  if (magic !== "FMIR" || version !== 2) {
    throw new Error(
      `IR guard failed: ${irName} is ${magic} v${version}, expected FMIR v2 ` +
        "(forma-server 0.1.4 contract)",
    );
  }
  if (!manifest.routes[route].js?.length) {
    throw new Error(
      `manifest route '${route}' has no client js — the island runtime cannot load`,
    );
  }
  console.log(`OK: ${route} → ${irName} is FMIR v2; island client bundle kept`);
}
console.log("OK: controller art vendored (pad-xbox.svg, pad-ds4.svg)");
