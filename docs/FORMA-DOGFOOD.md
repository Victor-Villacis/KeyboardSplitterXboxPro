# Forma dogfood ledger

ksx Studio is the first production consumer of Forma on Windows (E7's
deliberate bet). Every real-world finding lands here with its status, so
nothing discovered in the trenches evaporates. Rule: a finding is either
FIXED-ADOPTED, an OPEN ASK (waiting on upstream), or OURS-TO-SEND (we owe
upstream the report/PR).

| # | Finding | Status | Where |
|---|---------|--------|-------|
| 1 | `@getforma/build` tailwind step spawned `npx` via `execFileSync` without `shell:true` → ENOENT on Windows (`npx` is `npx.cmd`) | **FIXED upstream** (build 0.1.9, 2026-08-05) — adopted same day; we still use plain-CSS entries by choice | `studio-ui/build.mjs` note |
| 2 | Compiler named every `createList` slot literally `list:array` — multi-list pages could not inject by name; we shipped a positional workaround | **FIXED upstream** (compiler 0.2.0, 2026-08-05) — adopted, workaround deleted | `crates/ksx-studio/src/render.rs` history note |
| 3 | 0.2.0's list names are document-order indexed (`list:#N:array`), not binding-derived (`list:<source>:array`) as the release notes suggested — reordering lists in the page still shifts names | **OPEN ASK** (binding-derived names); our pinning test turns drift into test failures meanwhile | `render.rs` seam constants doc |
| 4 | `createShow` slots are all named `show:createShow` — Bool show/hide state cannot be injected by name; SHOW_ORDER positional seam remains (16 entries deep after the design pass) | **OPEN ASK** (per-instance show naming) — the biggest remaining seam fragility | `render.rs` SHOW_ORDER |
| 5 | Plain hydration clobbers server-rendered values: `adoptNode` binds text to client signal state (first effect run overwrites SSR text with signal defaults) and list adoption removes SSR rows absent from the client array. The sanctioned path is the islands protocol (`data-forma-props` initializing signals BEFORE adoption) — discoverable only by reading `@getforma/core` internals | **OURS-TO-SEND**: docs ask — "SSR with server data" needs a documented recipe; ideally `hydrate()` could adopt DOM state as initial signal values | canon study, design-pass report |
| 6 | `create-forma-app` dashboard template binds `0.0.0.0` with no auth and no warning (its own sibling, the minimal template, computes a CSP then discards it — earlier E7 finding) | **OURS-TO-SEND**: security nudge — default to `127.0.0.1`, make LAN opt-in | E7; canon study |
| 7 | Good news worth telling upstream: FMIR format v2 held across five months of npm-side drift (core 1.5.0 vs Rust crates 0.1.4) — the binary contract is stable; `AssetManifest` deserialized byte-for-byte | evidence for a compat-guarantee doc | `docs/research/forma-spike-1-fmir-compat.md` |

Process note: findings 1+2 were reported through Victor and fixed upstream
overnight — found in production Tuesday, fixed and adopted Wednesday. The
loop works; keep feeding it.
