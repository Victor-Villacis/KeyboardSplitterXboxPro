import { StartIsland } from "./StartIsland";

// The SSR root — a compile-time artifact with one job and zero runtime
// presence, exactly like PadsPage.ts. Returning `StartIsland()` makes the whole
// screen one island: the compiler inlines its h() tree between
// ISLAND_START/ISLAND_END, so the Rust walker SSRs the full page and stamps the
// island attributes the client runtime activates against. `parseEntryPoint`
// picks the imported `*Page` that is NOT in start.ts's `activateIslands`
// registry as the SSR root, so this file must keep existing and keep returning
// the island.
//
// DO NOT ADD A `createSignal` HERE. It would mint a slot nothing renders and
// push the RENDERED one to a `#2` suffix, so injecting by name would fill the
// dead one and the page would show its compile-time default forever — with
// every test still green. See StatusPage.ts's longer note, docs/FORMA-DOGFOOD.md
// #9, and `assert_island_slot_contract` in render.rs.

export function StartPage() {
  return StartIsland();
}
