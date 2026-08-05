// Geometry extractor for the vendored Gamepad-Asset-Pack SVGs.
//
// PadForge lesson (docs/research/padforge-code-audit.md §1.4): derive layout
// data from the art with a script, never hand-edit. This script parses each
// pad SVG, walks every path/circle, applies the layer translate, and prints
// per-element bounding boxes as FRACTIONS of the viewBox — the numbers the
// hit-zone tables in ../src/MapIsland.ts (and their Rust mirror) are authored
// from. Rough on purpose: curve control points are treated as extent points,
// which slightly over-estimates a bbox but never misplaces its center.
//
// Usage: node extents.mjs src-xbox360.svg

import { readFileSync } from "fs";

const file = process.argv[2] ?? "src-xbox360.svg";
const svg = readFileSync(new URL(file, import.meta.url), "utf8");

const viewBox = /viewBox="([^"]+)"/.exec(svg)[1].split(/\s+/).map(Number);
const [vx, vy, vw, vh] = viewBox;

// Layer translate (both icons carry one top-level translate).
const tr = /transform="translate\(([-\d.]+),([-\d.]+)\)"/.exec(svg);
const [tx, ty] = tr ? [Number(tr[1]), Number(tr[2])] : [0, 0];

function pathBBox(d) {
  // Tokenize commands; track the current point; collect every coordinate the
  // path touches (control points included — extent-approximate).
  const tokens = d.match(/[a-zA-Z]|-?\d*\.?\d+(?:e-?\d+)?/g) ?? [];
  let i = 0, cmd = "", x = 0, y = 0, sx = 0, sy = 0;
  const xs = [], ys = [];
  const num = () => Number(tokens[i++]);
  const point = (px, py) => { xs.push(px); ys.push(py); };
  while (i < tokens.length) {
    if (/[a-zA-Z]/.test(tokens[i])) cmd = tokens[i++];
    const rel = cmd === cmd.toLowerCase();
    switch (cmd.toLowerCase()) {
      case "m": case "l": case "t": {
        const nx = num(), ny = num();
        x = rel ? x + nx : nx; y = rel ? y + ny : ny;
        point(x, y);
        if (cmd.toLowerCase() === "m") { sx = x; sy = y; cmd = rel ? "l" : "L"; }
        break;
      }
      case "h": { const nx = num(); x = rel ? x + nx : nx; point(x, y); break; }
      case "v": { const ny = num(); y = rel ? y + ny : ny; point(x, y); break; }
      case "c": {
        const c = [num(), num(), num(), num(), num(), num()];
        const [x1, y1, x2, y2, nx, ny] = rel ? [x + c[0], y + c[1], x + c[2], y + c[3], x + c[4], y + c[5]] : c;
        point(x1, y1); point(x2, y2); x = nx; y = ny; point(x, y);
        break;
      }
      case "s": case "q": {
        const c = [num(), num(), num(), num()];
        const [x1, y1, nx, ny] = rel ? [x + c[0], y + c[1], x + c[2], y + c[3]] : c;
        point(x1, y1); x = nx; y = ny; point(x, y);
        break;
      }
      case "a": {
        num(); num(); num(); num(); num();
        const nx = num(), ny = num();
        x = rel ? x + nx : nx; y = rel ? y + ny : ny; point(x, y);
        break;
      }
      case "z": x = sx; y = sy; break;
      default: i++; // unknown token — skip
    }
  }
  if (!xs.length) return null;
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

const frac = (n, base, span) => ((n - base) / span * 100).toFixed(1);

for (const m of svg.matchAll(/<(path|circle)\b[^>]*>/gs)) {
  const el = m[0];
  const id = (/id="([^"]+)"/.exec(el) ?? [, "?"])[1];
  let box = null;
  if (m[1] === "circle") {
    const cx = Number(/cx="([^"]+)"/.exec(el)[1]);
    const cy = Number(/cy="([^"]+)"/.exec(el)[1]);
    const r = Number(/r="([^"]+)"/.exec(el)[1]);
    box = [cx - r, cy - r, cx + r, cy + r];
  } else {
    const d = /\bd="([^"]+)"/.exec(el);
    if (d) box = pathBBox(d[1]);
  }
  if (!box) continue;
  const [x0, y0, x1, y1] = box;
  // Apply the layer translate, then normalize to viewBox %.
  console.log(
    `${m[1]} ${id}: x ${frac(x0 + tx, vx, vw)}..${frac(x1 + tx, vx, vw)}%  ` +
    `y ${frac(y0 + ty, vy, vh)}..${frac(y1 + ty, vy, vh)}%  ` +
    `center ${frac((x0 + x1) / 2 + tx, vx, vw)},${frac((y0 + y1) / 2 + ty, vy, vh)}`
  );
}
console.log(`viewBox ${vw} x ${vh} (aspect ${(vw / vh).toFixed(4)})`);
