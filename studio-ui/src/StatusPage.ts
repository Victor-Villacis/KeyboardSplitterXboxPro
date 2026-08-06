import { StatusIsland } from "./StatusIsland";

// The SSR root — a compile-time artifact, ONE job now, zero runtime presence:
//
// **Island anchor.** Returning `StatusIsland()` makes the whole screen one
// island: the compiler inlines its h() tree between ISLAND_START/ISLAND_END,
// so the Rust walker SSRs the full page (first paint needs no JS) and stamps
// the island attributes the client runtime activates against.
// `parseEntryPoint` (component-analyzer.ts) picks the imported `*Page` that is
// NOT in status.ts's `activateIslands` registry as the SSR root, so this file
// must keep existing and keep returning the island.
//
// **What used to be here** (deleted 2026-08-06, docs/FORMA-DOGFOOD.md #9):
// 29 `createSignal` re-declarations — 12 scalars + 17 booleans — that existed
// ONLY to put named slots into the IR, because `@getforma/compiler` 0.2.0
// extracted signal defaults from the root `*Page` component function alone.
// Compiler 0.3.1 walks island component files too (walk-driven signal
// scopes), so StatusIsland.ts's own declarations mint the named slots. Keeping
// the twins after 0.3.1 is actively WRONG: each twin declaration minted its
// own slot and pushed the island's real one to a `#2` suffix, so injecting
// `vigemLine` by name would fill a slot nothing renders. Removing them took
// the status IR from 96 slots (29 dead) back to 67 — no signal name suffixed
// (`list:profileRows#2:*` is the intended second occurrence of one list).
//
// DO NOT ADD A `createSignal` BACK. Verified 2026-08-06: one resurrected twin
// passed all 97 Rust tests while the page rendered a compile-time default.
// `build.mjs` now throws on the compiler's collision warning, and
// `render.rs`'s `assert_island_slot_contract` requires every injected name to
// be a slot the island actually renders.
//
// This function never executes in a browser — the client bundle runs
// `activateIslands` (status.ts), which builds the tree via StatusIsland
// directly. esbuild is free to tree-shake it.

export function StatusPage() {
  return StatusIsland();
}
