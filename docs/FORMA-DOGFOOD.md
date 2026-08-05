# Forma dogfood ledger

ksx Studio is the first production consumer of Forma on Windows (E7's
deliberate bet). Every real-world finding lands here with its status, so
nothing discovered in the trenches evaporates. Rule: a finding is either
FIXED-ADOPTED, an OPEN ASK (waiting on upstream), or OURS-TO-SEND (we owe
upstream the report/PR).

**Upstream repo map** (file bugs here; verified 2026-08-05):
- `@getforma/compiler`, `@getforma/build` -> github.com/getforma-dev/forma-tools
- `@getforma/core` (FormaJS runtime) -> getforma-dev org (verify exact repo when filing)
- `forma-server` / `forma-ir` (Rust crates, 0.1.4) -> github.com/getforma-dev/forma-server
- `@getforma/create-app` -> github.com/getforma-dev/create-forma-app
- kmd (docs dashboard; not implicated in any finding) -> github.com/getforma-dev/kmd

| # | Finding | Status | Where |
|---|---------|--------|-------|
| 1 | `@getforma/build` tailwind step spawned `npx` via `execFileSync` without `shell:true` → ENOENT on Windows (`npx` is `npx.cmd`) | **FIXED upstream** (build 0.1.9, 2026-08-05) — adopted same day; we still use plain-CSS entries by choice | `studio-ui/build.mjs` note |
| 2 | Compiler named every `createList` slot literally `list:array` — multi-list pages could not inject by name; we shipped a positional workaround | **FIXED upstream** (compiler 0.2.0, 2026-08-05) — adopted, workaround deleted | `crates/ksx-studio/src/render.rs` history note |
| 3 | 0.2.0's list names are document-order indexed (`list:#N:array`), not binding-derived (`list:<source>:array`) as the release notes suggested — reordering lists in the page still shifts names | **MOSTLY RESOLVED for us (v4, 2026-08-05)**: binding-derived names DO exist when the list source is a derivable binding — `() => padTiles()` compiles to `list:padTiles:array` (occurrence-suffixed `#N` on reuse); only literal sources like `() => []` fall back to positional. v4's signal-backed lists get named slots for free. Residual ask: document it; derive something stabler than doc-order for literals | `render.rs` seam constants doc |
| 4 | `createShow` slots are all named `show:createShow` — Bool show/hide state cannot be injected by name; SHOW_ORDER positional seam remains (16 entries deep after the design pass) | **OPEN ASK** (per-instance show naming) — the biggest remaining seam fragility | `render.rs` SHOW_ORDER |
| 5 | Plain hydration clobbers server-rendered values: `adoptNode` binds text to client signal state (first effect run overwrites SSR text with signal defaults) and list adoption removes SSR rows absent from the client array. The sanctioned path is the islands protocol (`data-forma-props` initializing signals BEFORE adoption) — discoverable only by reading `@getforma/core` internals | **ADOPTED in production (v4, 2026-08-05)**: Studio is now one island whose signals seed from props before adoption, then a 2 s `/api/status` poller — live updates, zero clobber. The docs ask ("SSR with server data" recipe) is still **OURS-TO-SEND** | `render.rs` module docs; `studio-ui/src/status.ts` |
| 6 | `create-forma-app` dashboard template binds `0.0.0.0` with no auth and no warning (its own sibling, the minimal template, computes a CSP then discards it — earlier E7 finding) | **OURS-TO-SEND**: security nudge — default to `127.0.0.1`, make LAN opt-in | E7; canon study |
| 7 | Good news worth telling upstream: FMIR format v2 held across five months of npm-side drift (core 1.5.0 vs Rust crates 0.1.4) — the binary contract is stable; `AssetManifest` deserialized byte-for-byte | evidence for a compat-guarantee doc | `docs/research/forma-spike-1-fmir-compat.md` |
| 8 | Compiler 0.2.0 registers every island with EMPTY `slot_ids` (`addIsland(name, trigger=load, propsMode=inline, [], offset)`), so `forma-ir`'s walker — which is fully wired to emit `data-forma-props` from `build_island_props(slot_ids)` — never fires. Server-side island props require hand-emitting the `__forma_islands` script block (which `loadIslandProps` accepts as its shared-props path) | **OPEN ASK**: populate `slot_ids` with the slots referenced inside the island span; our layout test asserts `slot_ids.is_empty()` so a fixed compiler flips the test and we adopt the native path | `render.rs` `island_props_json` / layout test |
| 9 | Signal slot-table extraction (`extractSignalDefaults`) reads ONLY the root `*Page` component function — signals declared in island component files get anonymous `text:N` slots, killing name-keyed injection. Forces the v4 split: StatusPage.ts re-declares all 27 signals as compile-time slot declarations while StatusIsland.ts owns the runtime twins — same names, two files, drift caught only by our layout test | **OURS-TO-SEND**: extract signal defaults from island component files too (or a documented single-source pattern) | `studio-ui/src/StatusPage.ts` header |
| 10 | An identifier as a static attr value (`d: SIL_BODY`, a module `const` string) is silently compiled to an empty SOURCE_CLIENT slot — SSR renders the element with the attribute MISSING, no build warning. Studio v1–v3 shipped pad silhouettes with no body path and nothing noticed until the v4 canon study; inlining the literal fixed it (was pinned by a `d="M20 7` SSR test until v5 replaced the silhouettes with vendored art) | **OURS-TO-SEND**: resolve module-level string consts (the compiler already folds `+` concatenations in `evalNode`) or at least warn | history: `StatusIsland.ts` (v4) |
| 11 | Good news, load-bearing for v5: **member-expression attribute values in `createList` item bodies compile to per-item dyn-attr slots** (`h("img", { src: p.art })`, `style: z.style`, `"data-fn": z.fn`) — SSR emits the attribute from the injected array, the client runtime re-derives it per item. The whole mapper zone layer (25 positioned buttons × style/class/data-fn/title) and the status tiles' per-persona art ride this; without it every zone would have needed its own named signal. Constraint held from #9's world: the member read must be a bare `param.field` (or `String(param.field)`) — computed/derived expressions still get anonymous slots | worth documenting upstream as a SUPPORTED pattern (it is compiler 0.2.0's `listItemBindings` path) | `MapIsland.ts` zone list; `render_map.rs` `zone_rows` |
| 12 | Two-route builds work end to end (entryPoints + routes + per-route `ssrEntryPoints`), including the twin-file pattern per page (#9 forces a `MapPage`/`MapIsland` split exactly like status) — but the island BYPRODUCT cleanup (#5's `*.islands.js`) must be repeated **per entry**, and the second occurrence of a reused list binding gets the `#N` suffix across a page, not across the build (each page's IR names are independent) | evidence for upstream docs; no bug | `build.mjs` (v5); `render_map.rs` `list:zones#2:array` |

## Details for filing — mechanism, upstream location, local repro

### #1 — build <=0.1.8: Windows tailwind ENOENT
**Upstream**: forma-tools -> `@getforma/build`, the tailwind cssEntries step.
**Error**: `spawnSync npx ENOENT` the moment a build has `tailwind: true`.
Root cause: `execFileSync("npx", ...)` without `shell: true`; on Windows
`npx` is `npx.cmd` (a cmd script, not a PE image), so CreateProcess cannot
exec it. **Repro**: any Windows box, any tailwind entry. **Fixed** in 0.1.9
(resolves local `@tailwindcss/cli`, runs it via node directly; npx fallback
shell-spawned with quoted args). Local note: `studio-ui/build.mjs`.

### #2 — compiler <=0.1.8: all list slots named `list:array`
**Upstream**: forma-tools -> compiler slot-table emission for `createList`.
**Error mode**: silent — with two lists on a page, name-keyed injection
(`SlotData::from_json`) addresses only "the" `list:array` slot; every other
list renders its compile-time default (usually empty). No warning anywhere.
**Repro**: page with two createLists, inject both by name, observe one
blank. **Fixed** in 0.2.0. History note: `crates/ksx-studio/src/render.rs`.

### #3 — compiler 0.2.0: literal-source lists still positional
**Upstream**: same emission path as #2. **Detail**: a derivable binding
source (`() => padTiles()`) earns `list:padTiles:array` (+`#N` on reuse);
a literal source (`() => []`) falls back to `list:#N:array` where N is
document order — reordering the page silently renames the slot and
injection misses. **Ask**: document both behaviors; stabler naming for
literals. Local guard: layout tests pin exact names (`render.rs`,
`render_map.rs`).

### #4 — compiler: every `createShow` slot is `show:createShow`
**Upstream**: forma-tools -> show/Bool slot emission. **Error mode**:
silent — show/hide state cannot be injected by name at all; N shows on a
page = N identically-named slots. **Cost here**: 16 Bool slots on the
status page + 12 on the mapper, all injected positionally via SHOW_ORDER
arrays that must mirror compile order exactly (drift = wrong pills/panels
showing, caught only by our pinning tests). **Ask**: mirror the #2 fix for
shows. Our single biggest remaining seam fragility. Local: `render.rs`
SHOW_ORDER, `render_map.rs` MAP_SHOW_ORDER.

### #5 — core: plain hydration clobbers server-rendered values
**Upstream**: `@getforma/core` -> `mount()` -> `hydrateIsland()` ->
`adoptNode()`. **Mechanism** (from reading core internals):
`setHydrating(true)` makes `h()` return descriptors; `adoptNode` attaches
bindings to the SSR DOM without creating elements — but bindings attach to
CLIENT signal state: text bindings are
`internalEffect(() => textNode.data = String(child()))` whose FIRST run
overwrites SSR text with the signal default, and list adoption reads
`untrack(() => child.items())` then REMOVES SSR rows absent from the
client array (console: "removing extra SSR list item"). Net: any
server-data SSR page visibly reverts to defaults on mount. **Sanctioned
path** (undocumented): the islands protocol — props via `data-forma-props`
or the `__forma_islands` JSON script block -> `activateIslands` hands
props to the hydrate fn BEFORE adoption -> signals seed from server
values. **Ask**: document the recipe; ideally let hydrate() adopt DOM
state as initial signal values. Local: `studio-ui/src/status.ts`,
`render.rs` module docs.

### #6 — create-app templates: insecure defaults
**Upstream**: create-forma-app -> dashboard template `src/main.rs` (binds
`0.0.0.0`, no auth, no warning) and the minimal template (computes a CSP
then discards it). **Ask**: default `127.0.0.1`; LAN as explicit opt-in;
apply the computed CSP. Contrast: our `server.rs` refuses non-loopback
binds in code and test.

### #7 — FMIR v2 stability (good news)
Compiler (npm, Jul 2026) emits magic `FMIR` + u16 LE version `2`;
`forma-ir` 0.1.4 (Mar 2026) expects exactly `IR_VERSION: u16 = 2`. Parse,
`check_ir_compatibility`, `render_page`, `AssetManifest` all clean across
the five-month gap (`docs/research/forma-spike-1-fmir-compat.md`); our
build asserts the magic+version bytes on every regen. Worth a published
compat guarantee.

### #8 — compiler: islands registered with EMPTY slot_ids
**Upstream**: forma-tools -> compiler emits
`addIsland(name, trigger=load, propsMode=inline, [], offset)` — the
slot_ids array is ALWAYS empty. The downstream half already exists:
`forma-ir`'s walker is fully wired to emit `data-forma-props` from
`build_island_props(slot_ids)` — dead code in practice. **Consequence**:
server-side island props require hand-emitting the `__forma_islands`
script block (`loadIslandProps`' shared-props path; non-executing JSON,
CSP-exempt, needs `<`-escaping against breakout). **Ask**: populate
slot_ids from the island span. Local tripwire: our layout test asserts
`slot_ids.is_empty()` — a fixed compiler flips the test and we adopt the
native path. Local: `render.rs` `island_props_json`.

### #9 — compiler: signal extraction reads only the root `*Page` fn
**Upstream**: forma-tools -> `extractSignalDefaults`. **Mechanism**: named
signal slots come ONLY from the root Page component's body; `createSignal`
in island component files -> anonymous `text:N` slots -> name-keyed
injection impossible where the signal naturally lives. **Cost**: the
twin-file pattern — StatusPage.ts/MapPage.ts re-declare 27/22 signals as
compile-time slot declarations while the Island files own runtime twins;
same names, two files, drift caught only by our tests. **Ask**: extract
from island files, or document a single-source pattern. Local:
`StatusPage.ts` header comment.

### #10 — compiler: identifier attr values silently drop the attribute
**Upstream**: forma-tools -> static-attribute evaluation (`evalNode`,
which already folds `+` concatenations but not identifier references).
**Mechanism**: `h('path', { d: SIL_BODY })` with SIL_BODY a module const
string compiles to an empty SOURCE_CLIENT slot -> SSR renders the element
with the attribute MISSING. No build warning, no runtime error — silent
wrong output. **Real cost**: Studio v1-v3 shipped pad silhouettes whose
`<path>` had no `d` for four versions before the v4 canon study caught
it. Highest-severity report in this ledger. **Ask**: resolve module-level
consts or at minimum warn. History: `StatusIsland.ts` (v4).

### #11 — member-expression attrs in list items (supported-pattern ask)
**Upstream**: forma-tools -> compiler 0.2.0 `listItemBindings` path.
**Behavior** (good, load-bearing): bare `param.field` reads as attr values
inside createList item bodies (`style: z.style`, `"data-fn": z.fn`,
`src: p.art`) compile to per-item dyn-attr slots — SSR emits from the
injected array, client re-derives per item. The whole mapper zone layer
(25 buttons x 4 attrs) rides this. Constraint: BARE member reads only;
computed expressions get anonymous slots (#9's world). **Ask**: confirm as
contract, not accident. Local: `MapIsland.ts`, `render_map.rs` zone_rows.

### #12 — build: multi-route facts (docs note)
Two-route builds work (entryPoints + routes + per-route ssrEntryPoints);
island byproduct cleanup (`*.islands.js`) must be repeated PER ENTRY; list
`#N` occurrence suffixes are per-page, not per-build. No bug — upstream
docs material. Local: `build.mjs` (v5).

### #13 — reserved (under diagnosis)
The mapper learn-flow freeze (second learn wedges the countdown; repeat
learn renders an empty flash box; tags do not update live after save) is
being diagnosed; if a same-value-signal-write reactivity quirk in
`@getforma/core` is underneath, the mechanism lands here. Ours-vs-theirs
verdict pending.

---
Process note: findings 1+2 were reported through Victor and fixed upstream
overnight — found in production Tuesday, fixed and adopted Wednesday. The
loop works; keep feeding it.
