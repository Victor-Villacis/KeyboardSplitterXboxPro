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
  // The remedy printed in the no-daemon banner, with this machine's profile
  // flag when it needs one.
  const [daemonCmd] = createSignal("ksx daemon");
  // The third restore destination's label carries its timestamp.
  const [backupLine] = createSignal("Restore backup");
  const [modalPrompt] = createSignal("");
  const [modalBinding] = createSignal("");
  const [countdownText] = createSignal("");
  const [barStyle] = createSignal("width:100%");
  const [conflictLine] = createSignal("");
  const [savedLine] = createSignal("");
  // Auto-save made visible; empty until this page has written something.
  const [savedAt] = createSignal("");
  const [generatedAt] = createSignal("(no snapshot)");
  // v7 multi-select (a JS enhancement — SSR always paints it off).
  const [selToggleCls] = createSignal("btn btn-row seltoggle");
  const [selToggleLabel] = createSignal("Select multiple");
  const [selCountLine] = createSignal("");
  // The preset-actions card renders inert until a payload proves the daemon
  // reachable (a class string, not a show — ledger #13).
  const [actionsCls] = createSignal("card pactions off");
  // Booleans behind the createShow pairs (positional show:createShow slots —
  // render_map.rs MAP_SHOW_ORDER pins the document order). Default false:
  // nothing renders until the server says otherwise.
  const [pillRunning] = createSignal(false);
  const [pillIdle] = createSignal(false);
  const [pillDown] = createSignal(false);
  const [pillPaused] = createSignal(false);
  const [noDaemon] = createSignal(false);
  const [sessionRunning] = createSignal(false);
  const [pausedBar] = createSignal(false);
  const [readOnly] = createSignal(false);
  const [canLearn] = createSignal(false);
  const [artXbox] = createSignal(false);
  const [artDs4] = createSignal(false);
  const [hasBackup] = createSignal(false);
  const [savedOk] = createSignal(false);
  const [savedErr] = createSignal(false);
  const [modalOpen] = createSignal(false);
  const [modalListening] = createSignal(false);
  const [modalBound] = createSignal(false);
  const [modalConflict] = createSignal(false);
  // Appended LAST, deliberately: a show inserted mid-document shifts every
  // show after it (ledger #14). See render_map.rs MAP_SHOW_ORDER.
  const [selBar] = createSignal(false);

  return MapIsland();
}
