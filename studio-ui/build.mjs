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
import { brotliCompressSync, gzipSync, constants as zlibConstants } from "zlib";

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
// Dogfood ledger #13, part 2: patch the ADOPTION-path show effect
// (`setupShowEffect` in @getforma/core's hydrate.ts, bundled into each entry).
// Upstream materializes a re-toggled show branch INSIDE its own
// internalEffect run, so every binding created there (reactive text, nested
// shows, lists) is owned by that run and disposed when the effect re-runs —
// in practice: the modal's prompt goes stale after the first close, a
// reopened flash renders an empty box, and a conflict dialog that first
// appears on a reopened modal never renders at all. The runtime-path
// createShow does it right (createRoot + untrack per branch); this rewrite
// gives the adoption path the same semantics via the `__ksxShowBranch`
// helper the entries install (map.ts / status.ts). Anchored replacements
// that MUST match exactly once — an upstream change fails the build loudly
// instead of silently reintroducing the bug.
// ---------------------------------------------------------------------------
function patchAdoptionShowSeam(file) {
  let src = readFileSync(file, "utf8");
  const patches = [
    {
      name: "branch creation (whenTrue/whenFalse)",
      pattern:
        /=(\w+)\?(\w+)\?\?(\w+)\.whenTrue\(\):\3\.whenFalse\?(\w+)\?\?\3\.whenFalse\(\):null/g,
      replace:
        "=$1?$2??globalThis.__ksxShowBranch(()=>$3.whenTrue())" +
        ":$3.whenFalse?$4??globalThis.__ksxShowBranch(()=>$3.whenFalse()):null",
    },
    {
      name: "branch ensureNode (descriptor → DOM)",
      pattern: /(\w+)!=null&&!\(\1 instanceof Node\)&&\(\1=(\w+)\(\1\)\)/g,
      replace: "$1!=null&&!($1 instanceof Node)&&($1=globalThis.__ksxShowBranch(()=>$2($1)))",
    },
  ];
  for (const { name, pattern, replace } of patches) {
    const hits = [...src.matchAll(pattern)];
    if (hits.length !== 1) {
      throw new Error(
        `ledger #13 patch anchor '${name}' matched ${hits.length}× in ${file} ` +
          "(expected exactly 1) — upstream changed; re-derive the patch",
      );
    }
    src = src.replace(pattern, replace);
  }
  writeFileSync(file, src);
  // The build precompressed the unpatched bundle; regenerate both variants.
  writeFileSync(
    `${file}.br`,
    brotliCompressSync(src, {
      params: { [zlibConstants.BROTLI_PARAM_QUALITY]: 11 },
    }),
  );
  writeFileSync(`${file}.gz`, gzipSync(src, { level: 9 }));
  console.log(`OK: ledger #13 show-seam patch applied to ${file}`);
}
for (const logical of ["map.js", "status.js"]) {
  const hashed = manifest.assets[logical];
  if (!hashed) throw new Error(`manifest lost its '${logical}' asset`);
  patchAdoptionShowSeam(join(outputDir, hashed));
}

// ---------------------------------------------------------------------------
// Vendored controller art (Gamepad-Asset-Pack, MIT, by AL2009man — see
// art/README.md): the committed sources are print-style assets — a hidden
// reference raster (display:none <image>, ~18 KB of base64), a SOLID BLACK
// body silhouette (fill:#000000) and white control shapes (fill:#ffffff),
// authored for light backgrounds. On the studio's dark ground the black body
// reads as a shapeless blob, so the copy step is a real transform now:
//
//   1. drop the hidden rasters, display:none leftovers, path-effect defs and
//      editor metadata (geometry untouched; also a big byte diet);
//   2. reclass the two source colors — fill:#000000 → .pad-body,
//      fill:#ffffff → .pad-detail (the DS4 touchpad → .pad-inset so the big
//      slab stays subtler than the buttons). Inline fill/stroke declarations
//      are stripped so the injected sheet wins;
//   3. inject a <style> sheet mapping those classes to the studio palette
//      (tokens mirrored from studio.css) with a prefers-color-scheme:light
//      override — SVG-internal media queries apply inside <img>, so ONE
//      asset serves both themes;
//   4. add the few controls the icons never drew (Xbox guide/view/menu, DS4
//      share/options/PS) as small .pad-detail shapes at the exact spots the
//      hit-zone tables expect (stage % → art mm → +layer translate).
//
// render.rs serves the results at /_assets/pad-xbox.svg and
// /_assets/pad-ds4.svg; the status page's .tileart uses the same files.
// ---------------------------------------------------------------------------

// Palette sheet: dark values echo --panel-2/--muted/--text territory from
// studio.css; light values echo its light scheme. vector-effect keeps the
// silhouette outline ~1.5 px whatever size the <img> renders at.
const PAD_SHEET =
  "<style>" +
  ".pad-body{fill:#1d2534;stroke:#8593ad;stroke-width:1.5;stroke-linejoin:round;vector-effect:non-scaling-stroke}" +
  ".pad-detail{fill:#c3cbdc}" +
  ".pad-inset{fill:#2c3549}" +
  "@media (prefers-color-scheme:light){" +
  ".pad-body{fill:#e7ebf2;stroke:#55617a}" +
  ".pad-detail{fill:#3c4660}" +
  ".pad-inset{fill:#d3d9e5}" +
  "}</style>";

// Inline declarations that would defeat the injected sheet on reclassed
// elements. Keeps fill-rule/display/opacity.
const DROP_DECL =
  /^(?:fill|fill-opacity|stroke|stroke-width|stroke-miterlimit|stroke-linecap|stroke-linejoin|color|-inkscape-stroke|paint-order)$/;

function reclassElement(el) {
  const styleAttr = /style="([^"]*)"/.exec(el);
  if (!styleAttr) return el;
  let cls = null;
  if (styleAttr[1].includes("fill:#000000")) cls = "pad-body";
  else if (styleAttr[1].includes("fill:#ffffff"))
    cls = /id="rect1268"/.test(el) ? "pad-inset" : "pad-detail";
  if (!cls) return el;
  const kept = styleAttr[1]
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d && !DROP_DECL.test(d.split(":")[0].trim()));
  const replacement =
    `class="${cls}"` + (kept.length ? ` style="${kept.join(";")}"` : "");
  return el.replace(styleAttr[0], replacement);
}

function cleanSvg(source, extraShapes) {
  return (
    readFileSync(join("art", source), "utf8")
      .replace(/<sodipodi:namedview[\s\S]*?\/>/g, "")
      .replace(/<metadata[\s\S]*?<\/metadata>/g, "")
      // The hidden trace rasters and any other display:none leftovers.
      .replace(/<image\b[^>]*>/g, "")
      .replace(/<(?:path|circle|rect)\b[^>]*display:none[^>]*>/g, "")
      .replace(/<inkscape:path-effect[\s\S]*?\/>/g, "")
      .replace(/\s+(?:inkscape|sodipodi):[\w-]+="[^"]*"/g, "")
      .replace(/\s+xmlns:(?:inkscape|sodipodi|xlink)="[^"]*"/g, "")
      .replace(/<!--[\s\S]*?-->/g, "")
      // Theme reclass + the palette sheet.
      .replace(/<(?:path|circle|rect|g)\b[^>]*>/g, reclassElement)
      .replace(/(<svg\b[^>]*>)/, `$1\n  ${PAD_SHEET}`)
      // Undrawn controls, injected inside the layer group.
      .replace(/<\/g>\s*<\/svg>/, `${extraShapes}</g></svg>`)
      .replace(/\n{3,}/g, "\n\n")
  );
}

// Xbox: guide (stage 50,27), view (44,39), menu (56,39) → art mm + layer
// translate (82.634594,166.69041). DS4: share (30,25.5), options (70,25.5),
// PS (50,63) → + (26.849948,130.35184). Stage→art: x% of viewBox width,
// (y−14)/0.86 % of viewBox height (ART_SHARE mapping in render_map.rs).
const XBOX_EXTRA =
  '<circle class="pad-detail" cx="138.86" cy="178.28" r="4.1"/>' +
  '<circle class="pad-detail" cx="132.11" cy="188.98" r="1.9"/>' +
  '<circle class="pad-detail" cx="145.61" cy="188.98" r="1.9"/>';
const DS4_EXTRA =
  '<rect class="pad-detail" x="59.56" y="137.55" width="2.2" height="5.0" rx="1.1"/>' +
  '<rect class="pad-detail" x="104.63" y="137.55" width="2.2" height="5.0" rx="1.1"/>' +
  '<circle class="pad-detail" cx="83.19" cy="171.68" r="2.9"/>';

const ART = [
  ["src-xboxseries.svg", "pad-xbox.svg", XBOX_EXTRA],
  ["src-ds4.svg", "pad-ds4.svg", DS4_EXTRA],
];
for (const [source, out, extra] of ART) {
  writeFileSync(join(outputDir, out), cleanSvg(source, extra));
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
