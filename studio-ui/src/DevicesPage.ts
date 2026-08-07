import { DevicesIsland } from "./DevicesIsland";

// The SSR root for `/devices` — a compile-time artifact, one job, zero runtime
// presence. Same shape as StatusPage.ts / MapPage.ts: returning the island
// makes the whole screen one island, so the compiler inlines its h() tree
// between ISLAND_START/ISLAND_END and the Rust walker SSRs the full page.
// `parseEntryPoint` picks the imported `*Page` that is NOT in devices.ts's
// `activateIslands` registry as the SSR root, so this file must keep existing
// and keep returning the island.
//
// DO NOT ADD A `createSignal` HERE. Compiler 0.3.1 walks island component
// files for signal scopes, so DevicesIsland.ts's own declarations mint the
// named slots; a twin declaration in this file would mint the UNSUFFIXED name
// for the DEAD one and push the rendered slot to `<name>#2`, at which point
// the seam injects a slot nothing renders and the page shows its compile-time
// default forever, silently. `build.mjs` throws on the compiler's collision
// warning and `render.rs`'s `assert_island_slot_contract` is the durable gate,
// but the cheapest fix is not writing it. See docs/FORMA-DOGFOOD.md #9.
//
// This function never executes in a browser — the client bundle runs
// `activateIslands` (devices.ts), which builds the tree via DevicesIsland
// directly. esbuild is free to tree-shake it.

export function DevicesPage() {
  return DevicesIsland();
}
