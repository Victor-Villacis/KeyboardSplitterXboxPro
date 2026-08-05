import { createSignal } from "@getforma/core";
import { StatusIsland } from "./StatusIsland";

// The SSR root — a compile-time artifact, two jobs, zero runtime presence:
//
// 1. **Slot table.** `@getforma/compiler` 0.2.0 extracts `createSignal`
//    defaults ONLY from the entry's root `*Page` component function, and
//    names FMIR slots after these getters when the island tree references
//    them (`() => vigemLine()`). Every declaration below therefore exists to
//    put a NAMED, honestly-defaulted slot into the compiled IR — the names
//    must match the runtime signals in StatusIsland.ts one for one. A rename
//    on either side surfaces as a missing-slot failure in ksx-studio's
//    `embedded_ir_slot_layout_matches_the_seam` test, not a blank page.
//    (Single-source declaration is not possible today: the analyzer reads
//    literal `const [name] = createSignal(literal)` statements in THIS
//    function body only — docs/FORMA-DOGFOOD.md finding #9.)
//
// 2. **Island anchor.** Returning `StatusIsland()` makes the whole screen
//    one island: the compiler inlines its h() tree between
//    ISLAND_START/ISLAND_END, so the Rust walker SSRs the full page (first
//    paint needs no JS) and stamps the island attributes the client runtime
//    activates against.
//
// This function never executes in a browser — the client bundle runs
// `activateIslands` (status.ts), which builds the tree via StatusIsland
// directly. esbuild is free to tree-shake it.

export function StatusPage() {
  // Scalars — one slot per line of the page. Defaults are what renders if
  // server injection ever misses a name: honest ("not collected"), never
  // fake data.
  const [generatedAt] = createSignal("(no snapshot)");
  const [vigemLine] = createSignal("not collected");
  const [interceptionLine] = createSignal("not collected");
  const [daemonYesNo] = createSignal("unknown");
  const [daemonDetail] = createSignal("not collected");
  const [autostartLine] = createSignal("not collected");
  const [padsSummary] = createSignal("not collected");
  const [profilesSummary] = createSignal("not collected");
  const [configRoot] = createSignal("(unknown)");
  const [sessionLine] = createSignal("not collected");
  const [flashLine] = createSignal("");
  // The remedy printed in the no-daemon banner, with this machine's profile
  // flag when it needs one.
  const [daemonCmd] = createSignal("ksx daemon");
  // Booleans behind the createShow pairs (injected positionally into the
  // show:createShow slots — render.rs SHOW_ORDER). Default false: nothing
  // renders until the server says otherwise.
  const [pillRunning] = createSignal(false);
  const [pillIdle] = createSignal(false);
  const [pillDown] = createSignal(false);
  const [noDaemon] = createSignal(false);
  const [flashOk] = createSignal(false);
  const [flashError] = createSignal(false);
  const [canStart] = createSignal(false);
  const [canStop] = createSignal(false);
  const [daemonDown] = createSignal(false);
  const [vigemOk] = createSignal(false);
  const [vigemWarn] = createSignal(false);
  const [icptBorrowed] = createSignal(false);
  const [icptAbsent] = createSignal(false);
  const [autostartOn] = createSignal(false);
  const [autostartOff] = createSignal(false);
  const [rowsLive] = createSignal(false);
  const [rowsPlain] = createSignal(false);

  return StatusIsland();
}
