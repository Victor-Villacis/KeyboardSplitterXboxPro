# Forma dogfood ledger

ksx Studio is the first production consumer of Forma on Windows (E7's
deliberate bet). Every real-world finding lands here with its status, so
nothing discovered in the trenches evaporates. Rule: a finding is either
FIXED-ADOPTED, an OPEN ASK (waiting on upstream), or OURS-TO-SEND (we owe
upstream the report/PR).

**Version in use** (2026-08-06): `@getforma/compiler` 0.3.1 and
`@getforma/build` 0.2.0, consumed as `file:` deps from a local clone of
forma-tools — npm publishing was down and only build 0.2.0 shipped;
`studio-ui/package.json` says when to swap back. `@getforma/core` 1.5.0 and the
Rust crates `forma-ir`/`forma-server` 0.1.4 are unchanged, which is why #13's
two local patches are still load-bearing.

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
| 4 | `createShow` slots were all named `show:createShow` — Bool show/hide state could not be injected by name; a SHOW_ORDER positional seam (17 status + 19 mapper entries) was the workaround | **FIXED upstream** (compiler 0.3.1, 2026-08-06) — **ADOPTED same day**: slots are named after the condition getter (`show:canStart`), `show_values()` yields `(name, bool)` pairs, both `*_SHOW_ORDER` arrays deleted, and the layout tests now assert the NAME SET instead of a count | `render.rs` / `render_map.rs` `show_values` |
| 5 | Plain hydration clobbers server-rendered values: `adoptNode` binds text to client signal state (first effect run overwrites SSR text with signal defaults) and list adoption removes SSR rows absent from the client array. The sanctioned path is the islands protocol (`data-forma-props` initializing signals BEFORE adoption) — discoverable only by reading `@getforma/core` internals | **ADOPTED in production (v4, 2026-08-05)**: Studio is now one island whose signals seed from props before adoption, then a 2 s `/api/status` poller — live updates, zero clobber. The docs ask ("SSR with server data" recipe) is still **OURS-TO-SEND** | `render.rs` module docs; `studio-ui/src/status.ts` |
| 6 | `create-forma-app` dashboard template binds `0.0.0.0` with no auth and no warning (its own sibling, the minimal template, computes a CSP then discards it — earlier E7 finding) | **OURS-TO-SEND**: security nudge — default to `127.0.0.1`, make LAN opt-in | E7; canon study |
| 7 | Good news worth telling upstream: FMIR format v2 held across five months of npm-side drift (core 1.5.0 vs Rust crates 0.1.4) — the binary contract is stable; `AssetManifest` deserialized byte-for-byte | evidence for a compat-guarantee doc | `docs/research/forma-spike-1-fmir-compat.md` |
| 8 | Compiler 0.2.0 registered every island with EMPTY `slot_ids`, so `forma-ir`'s walker — fully wired to emit `data-forma-props` from `build_island_props(slot_ids)` — never fired; server-side island props needed a hand-emitted `__forma_islands` script block | **FIXED upstream** (compiler 0.3.1, 2026-08-06) — **ADOPTED same day**: 45 slot_ids on `/`, 186 on `/map`; `island_props_json` and the `__forma_islands` impersonation are deleted and the tripwire now asserts `!slot_ids.is_empty()` PLUS that every server-injected slot is in there. What it does NOT solve is in **#19** | `render.rs` `payload_json` / layout tests |
| 9 | Signal slot-table extraction read ONLY the root `*Page` component function, so signals declared in island files got anonymous `text:N` slots. Forced the twin-file split: `*Page.ts` re-declared every signal purely to mint slots while `*Island.ts` owned the runtime copies | **FIXED upstream** (compiler 0.3.1 walk-driven signal scopes, 2026-08-06) — **ADOPTED same day**: 29 twins deleted from StatusPage.ts, 67 from MapPage.ts; both files are now four lines plus (for MapPage) the #17 const tables. After 0.3.1 the twins were actively HARMFUL — see the adoption note below. **Hardened the same day**: a resurrected twin passed the entire 97-test gate, so the guard is no longer the build log — `build.mjs` throws on the collision and `assert_island_slot_contract` requires every injected name to be one the ISLAND RENDERS | `StatusPage.ts`, `MapPage.ts`, `render.rs` `assert_island_slot_contract` |
| 10 | An identifier as a static attr value (`d: SIL_BODY`, a module `const` string) compiled to an empty SOURCE_CLIENT slot — SSR rendered the element with the attribute MISSING, no warning. Studio v1–v3 shipped pad silhouettes with no body path for four versions | **FIXED upstream** (compiler 0.3.1, 2026-08-06) — **VERIFIED HERE** with a throwaway probe: `h("path", { d: PROBE_D })` now folds the const into a static attribute (string table holds the literal, no slot, no island). Nothing in ksx still needs the inlining workaround; the vendored art replaced the silhouettes in v5 | probe recorded below |
| 11 | Good news, load-bearing for v5: **member-expression attribute values in `createList` item bodies compile to per-item dyn-attr slots** (`h("img", { src: p.art })`, `style: z.style`, `"data-fn": z.fn`) — SSR emits the attribute from the injected array, the client runtime re-derives it per item. The whole mapper zone layer (25 positioned buttons × style/class/data-fn/title) and the status tiles' per-persona art ride this; without it every zone would have needed its own named signal. Constraint held from #9's world: the member read must be a bare `param.field` (or `String(param.field)`) — computed/derived expressions still get anonymous slots | worth documenting upstream as a SUPPORTED pattern (it is compiler 0.2.0's `listItemBindings` path) | `MapIsland.ts` zone list; `render_map.rs` `zone_rows` |
| 12 | Two-route builds work end to end (entryPoints + routes + per-route `ssrEntryPoints`) — but the island BYPRODUCT cleanup (`*.islands.js`) had to be repeated **per entry**, and the second occurrence of a reused list binding gets the `#N` suffix across a page, not across the build | **HALF FIXED** (build 0.2.0 no longer emits the byproducts; scrub deleted 2026-08-06, replaced by a build-time assertion that they stay gone). The `#N`-is-per-page fact stands, and the twin-file half is void now that #9 is fixed | `build.mjs`; `render_map.rs` `list:zones#2:array` |
| 13 | TWO core/server findings behind the mapper learn-flow bugs (2026-08-05): **(a)** forma-server's CSP nonce-locks `style-src`, and per spec a nonce there makes browsers ignore inline-style semantics — every `style=""` ATTRIBUTE dies, including the ones forma's own compiled list bindings emit (#11!): all 25 zone hit-areas collapsed into a 2 px pile at the stage's top-left (the "corrupted glyph"), and the countdown bar's width never moved. **(b)** `@getforma/core`'s ADOPTION-path show effect (`setupShowEffect` in hydrate.ts) materializes re-toggled branches inside its own `internalEffect` run with no `createRoot`/`untrack` — every binding created there is owned by that run and disposed on the next re-run: stale modal prompts on reopen, empty flash boxes, a conflict dialog that never renders (Victor's frozen-modal repro). The runtime-path `createShow` already does it right | **WORKED AROUND locally, OURS-TO-SEND both**: (a) `relax_style_src` rewrites the header to `style-src 'self' 'unsafe-inline'` (scripts stay nonce-locked); (b) `build.mjs` patches the compiled `setupShowEffect` to route branch creation through an unowned root (`__ksxShowBranch`, installed by both entries) — anchored replacements that fail the build if upstream drifts | `crates/ksx-studio/src/server.rs` `relax_style_src`; `studio-ui/build.mjs` ledger-#13 patch; `map.ts`/`status.ts` helper |
| 14 | **CLOSED WITH #4** (2026-08-06); kept because the measurement is the argument that closed it. The `show:createShow` positional seam did not just risk drift — it **priced** UI work. The v6 pass added 6 shows to the mapper (18) and 1 to the status page (17); each one is a four-file edit (island tree, twin Page declaration, `*_SHOW_ORDER` label, boolean position in `show_values`) whose only guard is our own count assertion, and an insertion in the MIDDLE of the document silently shifts every show after it. Named show slots would make all four edits one. **v7 update**: the whole multi-select feature cost exactly ONE new show (19) — because it was APPENDED as the last element of the document rather than placed where it belongs visually (it is `position: fixed`, so it could be). That is the seam shaping the markup: a bar that renders at the bottom of the page is written at the bottom of the tree to avoid renumbering 15 booleans | reinforces the **OPEN ASK** in #4 with a cost measurement: 3 pages of seam is no longer hypothetical, and it now influences where elements are authored | `render_map.rs` MAP_SHOW_ORDER (19); `render.rs` SHOW_ORDER (17) |
| 15 | Good news, and the pattern that made the v6 mapper affordable: **a per-item member read can carry a whole interaction**, not just presentation. The legend's ✕ clear accelerator is `h("span", { class: "lclear", "data-clear": l.fn, title: l.cleartitle }, l.clear)` — a bare `param.field` per #11, so SSR emits it and the client re-derives it per poll, and `map.ts` reads `data-clear` by delegation. Empty string = CSS-hidden, which is how "only offer the ✕ where clearing does something" costs zero shows. Same trick disables controls: `cls` carries `z-dead`/`l-dead` instead of a `disabled` attribute (which would swallow the click that owes the user an explanation) | confirms #11's contract holds for interaction attrs too — worth including in the upstream ask | `MapIsland.ts` legend list; `render_map.rs` `legend_rows` |
| 16 | Ledger #13(b)'s patched show seam held under a much heavier load: the v6 mapper nests shows THREE deep inside a re-toggled branch (`modalOpen` → `modalListening` / `modalBound` / `modalConflict`) and re-toggles them dozens of times per session with no stale prompts or empty boxes. The `__ksxShowBranch` unowned-root rewrite is the reason; without it this design would have been unbuildable | evidence for the upstream fix's shape (**OURS-TO-SEND**, unchanged) | `build.mjs` ledger-#13 patch |
| 17 | Good news with a catch, and what made v9's no-JavaScript mapper buildable: **`...CONST.map((x) => h(…))` spreads are EXPANDED AT COMPILE TIME** into static markup (`extractFileConstants` → `emitSpreadChild` → `substituteProperties`), so a 122-option `<select>` can be authored once and rendered on all 25 legend rows without a nested list (which the item-body seam does not offer) and without 122 literals per select. Three constraints, all silent when broken: (a) the constants are read from the **root `*Page` file only** — the same blind spot as #9 — and from plain top-level `const` declarations, so `export const` is invisible (declare bare, `export { … }` separately, import back from the island: single-sourced, unlike the signals); (b) substitution reaches **children only, not attribute values** — `h("option", { value: x.k }, x.t)` compiles the text but leaves `value` as an EMPTY dyn-attr slot, i.e. `<option value="">` on every option, which for a form is silently wrong data rather than missing decoration (we render `h("option", null, x.k)` and lean on HTML's option-text-is-the-value rule); (c) an unexpandable spread falls back to an ISLAND placeholder — the tell is the build line, `24 islands` where the page has one | **EDGES 1 AND 2 FIXED upstream** (compiler 0.3.1, 2026-08-06 — verified here by probe): constants declared in an ISLAND file now expand, and member reads now substitute into ATTRIBUTE values, so `h("option", { value: x.k }, x.t)` finally emits the value. Edge 3 (an unexpandable spread degrades to an island placeholder with no warning) is still **OURS-TO-SEND**. ksx keeps the tables in MapPage.ts for now — they work, and moving 250 lines buys nothing | `studio-ui/src/MapPage.ts` key tables; `MapIsland.ts` legend row form |
| 18 | Good news, and the reason v16's diagonal grid cost nothing structurally: **a list can grow a NEW SIBLING list without touching the show seam at all.** The macro grid's group band is a fourth `createList` inside `.macscroll` (`list:macroGroups:array`), and because 0.2.0 names lists from their binding (#3), adding it in the middle of the document renamed nothing — the only edit was one constant and one line in the seam-order assertion, versus #14's four-file cost for a single `createShow`. It also confirms #11/#15 one more time under a 37-column grid: `{ class: g.cls }` spans the band by CLASS (`macgrp g9`) rather than an inline `grid-column`, which #13(a) would have eaten anyway | reinforces #4's ask by CONTRAST: named list slots are already what named show slots should be | `render_map.rs` `LIST_SLOT_MACRO_GROUPS`; `MapIsland.ts` `.macgrps` |
| 19 | **The gap native island props leave.** With #8 fixed, `data-forma-props` carries the RENDERED SLOT VALUES (`vigemLine`, `show:canStart`, `list:padTiles:array`). ksx's clients own a MODEL over the SOURCE payload — `map.ts` keeps `lastPayload` and derives conflicts, macro drafts, undo and the learn flow from it — and no slot carries that, so a data block is still needed. Second half: inline props are not optional and not small. The walker emits them whenever `slot_ids` is non-empty and the compiler always picks `propsMode=Inline`, so `/map` gained a **106 557-character** attribute on a 320 397-character page (33%) — an escaped duplicate of markup already on the page, which this app cannot use and cannot turn off | **OURS-TO-SEND**: (a) a documented channel for server→island data that is not a slot value, or a way to declare extra props; (b) let the page choose `ScriptTag` props mode (forma-ir already implements it) or skip emission when nothing reads it | `render.rs` `PAYLOAD_SCRIPT_ID`; `status.ts`/`map.ts` `embeddedPayload()` |
| 21 | **The two derivations of every page have no gate.** #5's design emits the same data twice — SSR slots and `__ksx-payload` — so every sentence is derived once in Rust and once in TypeScript with nothing checking that they match. Verified drift: with a session RUNNING, `/map` painted `Use "Pause emulation & map" above` and hydration replaced it with `Use the Pause button above` — a visible flash whose surviving copy named a control that is not on the page | **OURS — FOUND AND FIXED 2026-08-06**: client wording corrected to the server's; `pwtest/ssr-hydration-parity.test.mjs` now diffs SSR against post-hydration DOM on 3 routes × 3 session states. Upstream half is #19's: a channel for server→island state would delete the second derivation | `MapIsland.ts` `reason()`; `render_map.rs` `reason_line` |
| 22 | The #20 fix folds a literal concatenation into an **anonymous slot's default** (`attr:title#2`, `text:0`) rather than into static markup the way a module const now does (#10) — so `/map` carries 5 extra slots, all inside the 106 KB props attribute, under document-order positional names; and a concatenation with a non-literal operand still lands there with an EMPTY default, which is #20(a) unchanged | **OURS-TO-SEND**: fold wholly-literal concatenations into the string table; warn when one is kept as a slot with an empty default. Locally pinned by `assert_island_slot_contract` (exact anonymous set + non-empty defaults) | `render_map.rs` `MAP_ANONYMOUS_SLOTS` |
| 20 | Two string-concatenation defects, reported 2026-08-05 and fixed overnight. **(a) attribute position**: `title: "a" + "b"` compiled to an empty slot — the attribute was emitted **with no value at all**, silently. Both 360 motion buttons and the macro Enable switch shipped with no tooltip until a pwtest case read the titles back and found `null`. **(b) child position**: `h("span", null, "a" + "b")` was a BinaryExpression the h-tree walker could not fold, so it emitted an anonymous SECOND ISLAND — caught only because our layout test asserts "exactly one island" | **FIXED upstream** (compiler 0.3.1) — **ADOPTED 2026-08-06**: both workarounds reverted (the one-giant-literal titles are wrapped concatenations again, `.macstephint` is one concatenated child), and `concatenated_strings_render_in_attributes_and_children` now pins the folded output so the silent failure mode cannot come back | `MapIsland.ts` motion buttons + `.macstephint`; `render_map.rs` |

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

### #4 — compiler: every `createShow` slot was `show:createShow`
**FIXED upstream (compiler 0.3.1) and ADOPTED 2026-08-06.** Show slots are now
named after the condition binding, so the whole positional apparatus below is
history. Kept because #14 and #18 priced it, and that measurement is what made
the ask land. What replaced it: `show_values()` returns `[(slot name, bool)]`,
`build_slots` looks each name up, and the layout tests assert the NAME SET on
both sides plus "no name occurs twice".
**Upstream**: forma-tools -> show/Bool slot emission. **Error mode**:
silent — show/hide state cannot be injected by name at all; N shows on a
page = N identically-named slots. **Cost here**: 17 Bool slots on the
status page + 18 on the mapper after the v6 pass, all injected positionally
via SHOW_ORDER arrays that must mirror compile order exactly (drift = wrong
pills/panels showing, caught only by our pinning tests). **Ask**: mirror the
#2 fix for shows. Our single biggest remaining seam fragility — and #14
prices it: every new show is a FOUR-file edit, and inserting one in the middle
of the document shifts every show after it. Local: `render.rs` SHOW_ORDER,
`render_map.rs` MAP_SHOW_ORDER.

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
**FIXED upstream (compiler 0.3.1) and ADOPTED 2026-08-06** — 45 slot_ids on
`/`, 186 on `/map`, and forma-ir's walker now emits `data-forma-props` itself.
`island_props_json` and the `__forma_islands` block are deleted. The tripwire
flipped exactly as designed and became the opposite assertion. Read **#19** for
what the fix does NOT cover.
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

### #9 — compiler: signal extraction read only the root `*Page` fn
**FIXED upstream (compiler 0.3.1, walk-driven signal scopes) and ADOPTED
2026-08-06.** 29 twin declarations deleted from `StatusPage.ts`, 67 from
`MapPage.ts`; both are now an import plus `return <Island>()`. Note the sting
in the tail: after 0.3.1 the twins were not merely redundant, they were WRONG
— see the adoption-pass table above.
**Upstream**: forma-tools -> `extractSignalDefaults`. **Mechanism**: named
signal slots come ONLY from the root Page component's body; `createSignal`
in island component files -> anonymous `text:N` slots -> name-keyed
injection impossible where the signal naturally lives. **Cost**: the
twin-file pattern — StatusPage.ts/MapPage.ts re-declare 27/22 signals as
compile-time slot declarations while the Island files own runtime twins;
same names, two files, drift caught only by our tests. **Ask**: extract
from island files, or document a single-source pattern. Local:
`StatusPage.ts` header comment.

### #10 — compiler: identifier attr values silently dropped the attribute
**FIXED upstream (compiler 0.3.1), VERIFIED HERE 2026-08-06** by probe: a
module-level `const` string used as `h("path", { d: PROBE_D })` now folds into
a static attribute. ksx has no live workaround left for this one (v5 replaced
the silhouettes with vendored art), so nothing to revert — but the mechanism
below is still the best write-up of why it was the highest-severity finding in
this ledger.
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
Two-route builds work (entryPoints + routes + per-route ssrEntryPoints); list
`#N` occurrence suffixes are per-page, not per-build. No bug — upstream docs
material. The island byproduct cleanup (`*.islands.js`, once needed PER ENTRY)
**stopped being necessary in build 0.2.0**: the files are not emitted at all.
The scrub came out on 2026-08-06 and left a one-line assertion in `build.mjs`
that fails the build if anything named `*.islands.*` reappears in the manifest
— cheaper than the scrub and louder than silence. Local: `build.mjs`.

### #13 — CSP kills style attributes; adopted shows dispose their branches
Diagnosed 2026-08-05 (Playwright repro against a mocked /api backend —
`learn-repro.mjs` / `conflict-repro.mjs` patterns; the daemon-side learn
state machine was verified INNOCENT: generations supersede correctly and a
second learn after a hit listens again, now pinned by
`a_second_learn_after_a_hit_listens_and_hits_again`).

**(a) forma-server CSP**: `PageOutput.csp` emits `style-src 'nonce-…'
'self'`. The CSP spec says a nonce/hash in a directive disables
`'unsafe-inline'` behavior — and inline `style=""` ATTRIBUTES can never
carry a nonce, so they are ALL dropped. But the compiler's own
`listItemBindings` path (#11) renders per-item `style:` attributes — the
mapper's entire zone geometry — and `MapIsland`'s countdown bar drives
`style: () => barStyle()`. Under the stock CSP the page quietly renders
every zone as a 2 px pile at the stage's top-left corner. **Ask**: either
stop nonce-locking style-src by default, or document that compiled style
bindings require `style-src 'unsafe-inline'`. Local fix:
`relax_style_src` in `server.rs` (scripts stay nonce-locked).

**(b) core hydrate.ts `setupShowEffect`**: the adoption-path show
materializes a branch inside its own `internalEffect` run —
`desc.whenTrue()` / `ensureNode(branch)` with no `createRoot`/`untrack` —
so the branch's reactive text, nested shows and lists are owned by that
effect run and are DISPOSED when it re-runs (the very next toggle). The
runtime-path `createShow` wraps branches in `createRoot(() =>
untrack(branchFn))`; the adoption path must do the same. Symptoms shipped:
modal reopens with the previous prompt, repeat flashes render an empty
box, a conflict dialog appearing on a reopened modal never renders (the
user-visible "second learn freezes" wedge). **Ask**: wrap adoption-path
branch creation in an unowned root. Local fix: `build.mjs` rewrites the
compiled seam (two anchored regex replacements, exactly-once enforced) to
call `globalThis.__ksxShowBranch` = `createUnownedRoot(() =>
untrack(make))`, installed by `map.ts`/`status.ts` before activation.
Trade-off documented: branches created this way are never disposed —
matching the seam's existing keep-the-fragment behavior, bounded by the
page's small show count.

### #14 — the show seam has a measured price now (v6, 2026-08-05)
The mapper's v6 pass (loud no-daemon banner, pause/resume for mapping, a third
restore destination, clear-one-in-modal) needed **6 new shows** on `/map` and
**1** on `/`. Each one costs four coordinated edits — the island's `h()` tree,
the twin `*Page.ts` compile-time declaration, the `*_SHOW_ORDER` label array,
and the boolean's POSITION in `show_values` — and an insertion in the middle of
the document (which is where a banner goes: first child of `<main>`) shifts
every show after it. Nothing but our own count assertion catches a mismatch,
and a mismatch is a wrong panel rendering, not an error. This is the same ask
as #4; what is new is the evidence that it is a recurring tax on UI work, not a
one-time setup annoyance.

### #15 — per-item member attrs carry INTERACTION, not just style (good news)
#11 established that bare `param.field` reads work as attribute values inside
`createList` item bodies. v6 leaned on that for behavior: the legend's clear
accelerator is `h("span", { class: "lclear", "data-clear": l.fn, title:
l.cleartitle }, l.clear)` — SSR emits the whole affordance, the client
re-derives it per poll, and `map.ts` picks it up by event delegation. Two
patterns fall out that cost ZERO extra shows: an empty string for the content
means "hide this per-row control" (CSS `:empty`), and a class-string field
(`z-dead` / `l-dead`) is how a control is rendered visibly disabled WITHOUT the
`disabled` attribute — which matters because a disabled button swallows its own
click, and a click on an unmappable control is exactly the click that owes the
user an explanation. Worth folding into the #11 upstream ask as the reason the
contract matters.

### #17 — compile-time spread expansion (and its three sharp edges)
Discovered building v9's no-JS mapper (2026-08-05). **Upstream**: forma-tools
→ `emitSpreadChild` / `extractFileConstants` / `substituteInExpr`.

**The good part**: `...KEYS.map((k) => h("option", null, k.k))` where `KEYS` is
a top-level array of object literals is unrolled INTO THE IR at build time —
static markup, no slots, no islands, and the identical source still runs in
the browser for the client re-render. That is the only reason a full 122-key
picker on 25 legend rows is affordable: the alternative was a nested
`createList` (the item-body seam has no such thing) or 3 000 hand-written
literals.

**Edge 1 — root-file only.** `extractFileConstants(componentSource)` is called
with the entry's resolved `*Page` file, and the island's walk context inherits
it unchanged (`islandWalkCtx = { ...walkCtx }`). Constants declared beside the
markup in `MapIsland.ts` are invisible; the spread silently degrades to an
island. It also scans `program.body` for `VariableDeclaration`, and an
`export const` is an `ExportNamedDeclaration`, so exporting the constant hides
it too. Local shape: declare bare in `MapPage.ts`, `export { … }` in a separate
statement, `import` back into `MapIsland.ts`. The resulting MapPage → MapIsland
→ MapPage cycle is inert (nothing reads the arrays before `MapIsland()` runs)
and, unlike #9's signals, this is SINGLE-SOURCED.

**Edge 2 — children only.** `substituteInCallExpr` walks the h() call's
arguments and substitutes bare member reads, but an ObjectExpression argument
(the props) is returned untouched. So `h("option", { value: k.k }, k.t)`
emits the text and then an `attr:value` slot with nothing in it: every option
renders `value=""`. For decoration that is invisible; for a FORM it is wrong
data submitted silently — the worst failure mode in this ledger after #10.
Local shape: no `value` attribute at all, and HTML's rule that an option's
trimmed text IS its value. (Which turned out to be the honest design anyway:
what you pick is character-for-character what the preset file will hold.)

**Edge 3 — the tell.** An unexpandable spread emits an island placeholder, so
the build's own line gives it away: `IR emitted (real): map.ir (13362 bytes,
24 islands)` for a page with exactly one island was the first symptom. Worth
a warning upstream; the byte count and island count are the only signal.

### #18 — a new sibling list is free; a new show is not (v16, 2026-08-06)
Diagonals-as-presentation replaced the macro grid's 25 columns with 37 (every
direction group is now its whole `↑ ↖ ← ↙ ↓ ↘ → ↗` ring) and added a group band
above the glyph row. The band is a fourth `createList` inserted in the MIDDLE of
`.macscroll`'s children.

Under #4's positional show seam that insertion would have renumbered every show
after it. Under 0.2.0's binding-derived list names (#3) it cost exactly two
lines: one `LIST_SLOT_MACRO_GROUPS` constant and one entry in the order the
layout test asserts. The lists around it — `macroCols`, `macroCells` — kept
their names because those names come from `() => macroCols()`, not from where
they sit.

That is the same feature, priced twice, in the same file: **#14 measured what a
show costs; this measures what a NAMED slot costs.** It is the strongest
argument we have for #4, because it is not hypothetical — it is the identical
edit in the identical page with the identical author.

Second confirmation, for #11/#15: the band spans its columns by CLASS
(`macgrp g9`, with `.macgrp.g9 { grid-column: span 9 }` in the stylesheet)
rather than an inline `grid-column`. That was not a style choice — #13(a) means
an inline `style` attribute would simply not survive the CSP — so per-item
member reads carrying LAYOUT, not just interaction, is now load-bearing too.

### #16 — the #13(b) patch held under three-deep nested shows
The v6 learn modal nests shows three levels inside a re-toggled branch
(`modalOpen` → `modalListening` / `modalBound` / `modalConflict`) and toggles
them dozens of times per mapping session. With upstream's adoption-path
`setupShowEffect` this would reproduce every symptom in #13(b) at once (stale
prompts, empty boxes, a dialog that never renders); with the `__ksxShowBranch`
unowned-root rewrite it is simply correct. Recorded because it strengthens the
upstream report: the bug is not an edge case, it is load-bearing for any modal.

### The 2026-08-06 adoption pass — measured, not assumed

`@getforma/compiler` 0.3.1 + `@getforma/build` 0.2.0, consumed from
`C:/Projects/forma-tools` as `file:` deps because npm publishing was down
(only build 0.2.0 made it out). `studio-ui/package.json` carries the note that
says when to swap back to `^0.3.1` / `^0.2.0`. **`@getforma/core` was not part
of this wave and stays at 1.5.0** — which is why #13(b) is still patched.

**Slot counts, before and after (real, from the emitted IR):**

| | compiler 0.2.0 (HEAD) | 0.3.1, twins still present | 0.3.1, twins deleted |
|---|---|---|---|
| `/` slots | 67 | **96** (29 dead) | **67** |
| `/map` slots | 216 | **283** (67 dead) | **216** |
| collision warnings | 0 | **96** | **0** |
| show slot names | `show:createShow` ×17 / ×19 | named | named |
| island `slot_ids` | 0 / 0 | 45 / 186 | 45 / 186 |

**Correction, same day** (measured off the committed `.ir` files, not the
build log): the right-hand column describes the tree *before* #20's
workarounds were reverted. Reverting them puts five ANONYMOUS slots back on
the mapper — four concatenated `title` attributes and one concatenated text
child, see #21 — so **the committed tree is `/map` 221 slots / 191 `slot_ids`**,
not 216 / 186. `/` is 67 / 45 either way. The 216 number is still the useful
one for the twin arithmetic (283 − 67); it just is not what ships.

The middle column is why deleting the twins was MANDATORY rather than tidy:
with 0.3.1 walking island files, each twin declaration minted the unsuffixed
slot and pushed the island's real one to `#2` (`generatedAt` vs
`generatedAt#2`), so every name-keyed injection would have filled a slot
nothing renders.

**And that guard was not the guard we thought it was.** This entry originally
said the compiler's per-collision warning made the failure self-diagnosing —
"a clean build has zero of those lines, which is a better guard than any
assertion we could write". Verified 2026-08-06 by resurrecting ONE twin
(`presetLine` in `MapPage.ts`), rebuilding, and running the whole gate:

- the compiler does warn, in `signal-scope.ts`, and the wording is good:
  *"signal 'presetLine' is also declared in another scope on this page, so this
  one gets the slot name 'presetLine#2' — inject it under that exact name"*;
- but it is **one `console.warn` line among the thirteen benign
  ArrayExpression notes this very document tells you not to chase**, `node
  build.mjs` exited 0, and **all 97 Rust tests passed**. The mapper rendered
  `(no preset)` for its preset name, forever, with a green gate.

`embedded_ir_slot_layout_matches_the_seam` could not catch it because it
asserted that the injected NAME EXISTS — and it does exist; it is the dead
one. Two guards were added instead (both verified to fail on the probe and
pass on the real tree):

1. `build.mjs` now **throws** on that warning rather than printing it;
2. `render::assert_island_slot_contract`, shared by both layout tests, checks
   the invariant that actually matters — **every name the seam injects must
   resolve to a slot the ISLAND RENDERS** (`slot_ids` membership), and every
   bare-named slot the island renders must be injected or listed as a
   documented client-only exception. Existence was never the contract.

**The real show names**, enumerated from the IR (this is the list the seam now
injects by name; document order shown, but nothing depends on it any more):

- `/` (17): `pillRunning pillIdle pillDown noDaemon flashOk flashError
  canStart canStop daemonDown rowsLive rowsPlain vigemOk vigemWarn
  icptBorrowed icptAbsent autostartOn autostartOff`
- `/map` (19): `pillRunning pillIdle pillDown pillPaused noDaemon
  sessionRunning pausedBar readOnly canLearn artXbox artDs4 hasBackup savedOk
  savedErr modalOpen modalListening modalBound modalConflict selBar`

Each is `show:` + the condition getter, so they are the signal names in
`*Island.ts`. The old positional order matched the new named mapping one for
one — checked before the swap — so this adoption changed no behaviour.

**What the handoff got wrong** (recorded because "verify every number" is the
rule that caught it):

- "~32 signal declarations in StatusPage.ts, ~68 in MapPage.ts" — really **29**
  and **67**.
- "roughly status 67, map 205" after the deletion — status 67 is right, map is
  **216**, exactly the 0.2.0 count.
- "~76 pre-existing `IR: signal …` warnings" — really **13**, all of them the
  ArrayExpression note on the nine list signals plus four more. The 0.3.1 build
  with twins present emits **109** lines, but 96 of those are the duplicate
  warnings the deletion removes.
- "the map route's 34 phantom islands should vanish on rebuild" — there were
  **none**. `/map` compiled to exactly 1 island before this pass and 1 after.
  The phantom islands were the shape of the #20(b) bug, and ksx's source had
  already been written around it (separate string children), so there was
  nothing to vanish. Reverting the workaround is what actually tested the fix.
- The show-name list was corrupted, as warned. `autostart…` is two names
  (`autostartOn`/`autostartOff`), and the map list omitted `pillPaused`.

**Probes run to settle the rest** (throwaway code in `StatusIsland.ts`, built,
IR inspected, reverted):

- #10: `const PROBE_D = "M20 7 L40 7 L40 27 Z"` used as `h("path", { d:
  PROBE_D })` — the literal lands in the string table beside the `d` key, no
  slot, no island. **Fixed.**
- #17 edge 1: the probe constants were declared in `StatusIsland.ts`, an
  ISLAND file, and the spread still expanded. **Fixed** — file constants are
  no longer root-`*Page`-only.
- #17 edge 2: `h("option", { value: o.v }, o.k)` emitted `AV`/`BV` as real
  static attribute values. **Fixed** — the silent-wrong-data edge is gone.
- #13(b): read `@getforma/core`'s `dist/chunk-3U2IQIKB.js`. `setupShowEffect`
  still materializes branches inside `internalEffect` with no
  `createRoot`/`untrack`, and both `build.mjs` patch anchors still match
  exactly once. **Not fixed — the `__ksxShowBranch` rewrite stays**, with a
  dated note saying why.

**Parity check** (Playwright, against the macro fixture): `/`, `/map` and
`/map?slot=1` loaded twice each, once with the app bundle blocked and once
with it live, DOMs diffed after hydration. Identical apart from four by-design
differences — forma's own `data-forma-status` lifecycle attribute
(`pending`→`active`), the client's `js` marker class, forma-ir's `U+200B`
placeholders for empty dynamic text slots, and `map.ts` writing `value=""` into
the form controls it owns. First paint matches what the client renders, which
was the risk in giving shows real SSR defaults.

**That check is a suite now, and its first real run found a flash** — see
**#21**. The manual pass above only ever ran the fixture's default state
(reachable + idle), and the disagreement lives in the RUNNING one.

### #19 — native island props carry slot values, not the domain payload
Adopting #8's fix answered the question we asked and raised a different one.
`build_island_props` maps `slot_ids` → slot names → current `SlotData`, so the
props object is `{"vigemLine": "…", "show:canStart": true,
"list:padTiles:array": […], "list:padTiles:art": …}` — the page's rendered
output, replayed. That is exactly right for seeding a presentational island and
exactly wrong for `/map`, whose client owns an editing MODEL: `lastPayload`
feeds conflict detection, the macro draft, undo, the learn state machine and
every "what would this write" preview. None of it is a slot.

So the `__forma_islands` impersonation died and a `__ksx-payload` block took
its place — same escaping, same parity tests, ksx's own name, and no pretence
that it is part of the islands protocol. `loadIslandProps` prefers the inline
attribute anyway, so the entries read their block directly by id rather than
through the `props` argument (which now arrives full of slot names).

The second half is cost. Inline props are emitted whenever `slot_ids` is
non-empty, and the compiler always writes `propsMode=Inline`; there is no
opt-out from the page's side. On `/map` that is a **106 557-character**
`data-forma-props` attribute — 33% of a 320 397-character response, an
HTML-escaped duplicate of markup already in the document, that this app never
reads. forma-ir already implements `PropsMode::ScriptTag` and already skips
emission for empty `slot_ids`; letting the author choose would cost upstream
almost nothing.

### #20 — the two concatenation defects (reported 2026-08-05, fixed overnight)
**(a) attribute position.** `title: "a" + "b"` compiled to an empty slot and
the attribute was emitted with NO VALUE. Silent: no warning, no error, just a
control with no tooltip — which is how both 360 motion buttons and the macro
Enable switch shipped without one. The tell was a pwtest case that read
`getAttribute("title")` back and got `null`.

**(b) child position.** `h("span", null, "a" + "b")` was a BinaryExpression the
h-tree walker could not fold, so it fell back to an anonymous ISLAND
placeholder — the same degradation as #17 edge 3. Only caught because
`embedded_map_ir_slot_layout_matches_the_seam` asserts "exactly one island";
without that assertion the page would have shipped with a second island whose
component nothing registers.

Both fixed in 0.3.1 and both workarounds reverted here, deliberately, as the
test: the motion titles are wrapped concatenations again and `.macstephint` is
one concatenated child. `concatenated_strings_render_in_attributes_and_children`
asserts the folded text reaches the SSR markup and that no `title=""` exists
anywhere on the page, because "the attribute is missing" is not something a
human notices in review.

### #21 — the mirror seam has no gate, and it had drifted (2026-08-06)
**OURS, not upstream's — and the most expensive shape left in this design.**

Every page here emits the same data twice on purpose (#5): slots for the SSR
paint, `__ksx-payload` for the client. That means two derivations of the same
sentence, in two languages, with nothing checking that they agree — `reason_line`
in `render_map.rs` against `reason()` in `MapIsland.ts`, `show_values` against
`applyMap`, and so on for every string on the page. A Rust test cannot see the
TypeScript; the compiler cannot see the Rust. Between them the disagreement is
invisible until it paints.

**What it had already cost.** With a session RUNNING, the mapper's read-only
banner painted

> read-only while emulation runs … Use **"Pause emulation & map"** above …

and then, on adoption, replaced it with

> read-only while emulation runs … Use **the Pause button** above …

A visible flash, and the surviving sentence was the wrong one: the button is
labelled "Pause emulation & map", so the hydrated copy named a control that
does not exist on the page. Fixed in `MapIsland.ts` (the server's wording won).

**The guard.** `studio-ui/pwtest/ssr-hydration-parity.test.mjs`: each route
(`/`, `/map`, `/map?slot=1`) is loaded twice — once with the app bundle
blocked at the network layer, so the island never hydrates, once normally —
and the island subtrees are diffed after normalizing exactly four by-design
differences (`data-forma-status`, the `js` class, `U+200B` text placeholders,
and the client's own empty `value=""`). It runs in all three session states
(`KSX_FIXTURE_SESSION=idle|running|down` — the fixture grew the switch for
this), because the drift was in the state nobody had loaded. 9 checks.

`modal open` is deliberately not among them: every modal show is injected
`false` unconditionally, so the modal cannot be part of a first paint and has
no SSR side to disagree with.

**The blind spot the guard still has, found the same day by driving a
TWO-SLOT cabinet** (the parity fixture has one slot, and one slot cannot
express this): the harness diffs the SSR paint against the HYDRATION seed, and
the seed is the payload the server embedded — so anything that only goes wrong
on the first `/api/map` POLL is invisible to it. `poll()` fetched `/api/map`
with no `slot=`, and `collect_map` falls back to the FIRST slot when the query
omits one. On `/map?slot=2` the SSR paint, the payload and the hydrated DOM all
agreed and were all correct; two seconds later the macro card silently swapped
to slot 1's macro table while the rail, the stage and the legend still said P2
— and `saveMacro` resolves the preset from the CLIENT's selection, so Save
would have written P1's steps into P2's preset. Fixed in `map.ts` (the poll
carries the selected slot). A parity test proves the first paint is honest; it
says nothing about the second one.

**Why it belongs in a Forma ledger even though the bug was ours**: the
duplicate derivation is not a ksx choice, it is what #5 leaves you with. A
framework that let the server hand its RENDERED state to the island (rather
than its slot values — #19) would delete the second derivation and this whole
class with it.

### #22 — the concat fix folds into a SLOT, not into markup
Reverting #20's workarounds (the whole point of the exercise: revert, and see
whether it still renders) shows what compiler 0.3.1 actually does with
`"a" + "b"`. It does not fold it into static markup the way #10's module const
now folds. It emits an **anonymous slot** whose DEFAULT holds the folded
string: `attr:title`, `attr:title#2`, `attr:title#3`, `attr:title#4` and
`text:0` on `/map` — the two 360 motion buttons, the reverse 360, the macro
Enable switch, and `.macstephint`.

That renders correctly, which is the fix. Three residues:

1. **They are the +5 in the count correction above** — 221 slots, and all five
   are in the island's `slot_ids`, so they are also five more entries in the
   106 KB `data-forma-props` attribute #19 is about. A page pays slot-table and
   props cost for a string that is constant at build time.
2. **The names are document-order positional** (`attr:title#3`), which is #3's
   complaint in a new place. Harmless only because nothing can inject them.
3. **The silent-empty shape survives one step over.** The default is the folded
   text only while every operand is a literal. `title: "a" + someSignal()` lands
   in the same anonymous slot with an EMPTY default — an attribute with no
   value, no warning — which is #20(a) exactly. Locally pinned:
   `render.rs`'s `assert_island_slot_contract` fails if the anonymous set
   changes, or if any anonymous slot's default is empty.

**Ask**: fold a wholly-literal concatenation into the string table like any
other literal (no slot, no props weight), and warn when a concatenation is
kept as a slot with nothing in its default.

---
Process note: findings 1+2 were reported through Victor and fixed upstream
overnight — found in production Tuesday, fixed and adopted Wednesday. The
second wave was bigger and the same shape: #4, #8, #9, #10, #17(edges 1+2) and
both halves of #20 were reported on the 5th and adopted on the 6th.

Still open, in the order they cost us: **#13(a)+(b)** (forma-server's CSP eats
`style` attributes; core's adoption-path show effect disposes its own branches
— both still patched locally, and core was NOT part of the 0.3.1 wave),
**#19**+**#21** (no channel for server→island data that is not a slot value,
inline props cannot be turned off, and the duplicate derivation that leaves is
now a measured flash rather than a theory), **#22** (a literal concatenation
becomes an anonymous slot instead of markup), **#17 edge 3** (an unexpandable
spread degrades to an island placeholder with no warning), **#3** (literal-source
list names are still document-order positional), **#5**/**#11**/**#15**/**#18**
(docs asks — the SSR-with-server-data recipe and the per-item member-read
contract), and **#6** (create-app's `0.0.0.0` default).

The lesson from the 0.3.1 verification pass, worth more than any single
finding: **every guard in this ledger that was a build-log line turned out not
to be a guard.** The collision warning was real, correctly worded, and
completely ineffective, because it printed next to thirteen warnings this file
told the reader to ignore. What holds is an assertion that fails the gate — and
the assertion has to test the thing that matters (does the island RENDER this
slot?), not the thing that is easy to test (does the name exist?). The loop
works; keep feeding it.
