import { createSignal } from "@getforma/core";
import { MapIsland } from "./MapIsland";

// The mapper's SSR root — same compile-time-only twin pattern as
// StatusPage.ts (docs/FORMA-DOGFOOD.md finding #9: the compiler extracts
// signal defaults ONLY from the entry's root *Page component function).
// Every declaration below exists to put a NAMED slot into the compiled IR;
// the names must match the runtime signals in MapIsland.ts one for one — a
// rename on either side fails ksx-studio's
// `embedded_map_ir_slot_layout_matches_the_seam` test, never a blank page.
//
// This function never executes in a browser (map.ts registers MapIsland via
// activateIslands; esbuild tree-shakes this).

export function MapPage() {
  // Scalars — defaults are what renders if server injection ever misses a
  // name: honest placeholders, never fake data.
  const [slotLine] = createSignal("no mappable slots");
  const [sourceLine] = createSignal("not collected");
  const [reasonLine] = createSignal("");
  const [cliLine] = createSignal("ksx map --preset <NAME> --function <FUNCTION> --key <KEY>");
  const [modalPrompt] = createSignal("");
  const [countdownText] = createSignal("");
  const [barStyle] = createSignal("width:100%");
  const [conflictLine] = createSignal("");
  const [savedLine] = createSignal("");
  const [generatedAt] = createSignal("(no snapshot)");
  // The preset-actions card renders inert until a payload proves the daemon
  // reachable (a class string, not a show — ledger #13).
  const [actionsCls] = createSignal("card pactions off");
  // Booleans behind the createShow pairs (positional show:createShow slots —
  // render_map.rs MAP_SHOW_ORDER pins the document order). Default false:
  // nothing renders until the server says otherwise.
  const [pillRunning] = createSignal(false);
  const [pillIdle] = createSignal(false);
  const [pillDown] = createSignal(false);
  const [readOnly] = createSignal(false);
  const [canLearn] = createSignal(false);
  const [artXbox] = createSignal(false);
  const [artDs4] = createSignal(false);
  const [savedOk] = createSignal(false);
  const [savedErr] = createSignal(false);
  const [modalOpen] = createSignal(false);
  const [modalListening] = createSignal(false);
  const [modalConflict] = createSignal(false);

  return MapIsland();
}
