import { SetupIsland } from "./SetupIsland";

// The SSR root for /setup — a compile-time artifact with one job, exactly like
// `StatusPage.ts`: returning `SetupIsland()` makes the whole screen one island,
// so the Rust walker SSRs the full page and stamps the attributes the client
// runtime activates against. `parseEntryPoint` picks the imported `*Page` that
// is NOT in setup.ts's `activateIslands` registry as the SSR root, so this file
// must keep existing and keep returning the island.
//
// DO NOT ADD A `createSignal` HERE. Read StatusPage.ts's comment for the
// evening it cost: a signal declared in two scopes gives the DEAD declaration
// the unsuffixed slot name and renames the RENDERED one to `<name>#2`, so the
// seam's injection fills a slot nothing renders and the page shows its
// compile-time default forever, with every test still green. `build.mjs` throws
// on the compiler's collision warning and `render.rs`'s
// `assert_island_slot_contract` is the durable gate; neither is a reason to
// find out again.
//
// This function never executes in a browser — esbuild is free to tree-shake it.

export function SetupPage() {
  return SetupIsland();
}
