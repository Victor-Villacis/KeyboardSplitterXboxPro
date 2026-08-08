import { ProfilesIsland } from "./ProfilesIsland";

// The SSR root — a compile-time artifact, one job, zero runtime presence.
// Same contract as StatusPage.ts / MapPage.ts: `parseEntryPoint`
// (component-analyzer.ts) picks the imported `*Page` that is NOT in
// profiles.ts's `activateIslands` registry as the SSR root, so this file must
// keep existing and keep returning the island.
//
// DO NOT ADD A `createSignal` HERE. It would mint the unsuffixed slot name for
// a declaration nothing renders and push the island's real one to `<name>#2`,
// so the seam's injection would silently fill a dead slot and this page would
// show its compile-time defaults forever. `build.mjs` throws on the compiler's
// collision warning and `render.rs`'s `assert_island_slot_contract` is the
// durable gate — but the cheapest guard is not writing it.
// See docs/FORMA-DOGFOOD.md #9 and StatusPage.ts's longer account.

export function ProfilesPage() {
  return ProfilesIsland();
}
