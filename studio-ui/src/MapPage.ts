import { MapIsland } from "./MapIsland";

// The mapper's SSR root — the same compile-time-only anchor as StatusPage.ts.
// `parseEntryPoint` picks the imported `*Page` that is NOT in map.ts's
// `activateIslands` registry as the SSR root, and `return MapIsland()` is what
// puts the whole screen between ISLAND_START/ISLAND_END.
//
// The 67 `createSignal` twins that used to live in `MapPage()` were deleted on
// 2026-08-06 (docs/FORMA-DOGFOOD.md #9): compiler 0.3.1 walks island component
// files for signal scopes, so MapIsland.ts's own declarations mint the named
// slots. After 0.3.1 the twins were harmful, not redundant — each minted its
// own slot and pushed the island's real one to `#2`, so injection by name hit
// a slot nothing renders (283 slots, 67 of them dead → 216).
//
// DO NOT ADD A `createSignal` BACK TO THIS FILE. It does not fail loudly on
// its own: the compiler renames the RENDERED binding to `<name>#2` and the
// seam then fills the dead one, so the page shows its authored default with a
// green gate. Two things now stop it — `build.mjs` throws on the compiler's
// collision warning, and `render.rs`'s `assert_island_slot_contract` requires
// every injected name to be a slot the island actually renders. (The committed
// mapper IR is 221 slots / 191 island slot_ids; the extra five over 216 are
// #21's anonymous concatenation slots, not signals.)
//
// The KEYS_*/FUNCTIONS tables below stay here by CHOICE, not by constraint.
// Compiler 0.3.1 fixed ledger #17 edge 1 — file constants declared in an
// ISLAND file expand too now (verified by probe, 2026-08-06) — so these 250
// lines could move to MapIsland.ts beside the markup that spreads them.
// Moving them buys nothing today and would rehash both bundles, so they stay;
// but nobody should preserve this arrangement believing the compiler still
// requires it. What they DO still prove is that the expansion works: the
// mapper SSRs 3 364 `<option>` elements across 34 selects as STATIC markup —
// no slots, no islands, no per-option literals in source.
//
// This function never executes in a browser (map.ts registers MapIsland via
// activateIslands; esbuild tree-shakes this).

// ── The no-JS vocabulary tables ────────────────────────────────────────────
// v9: the mapper's write path must work with JavaScript OFF. Learning a key
// needs a poller, so the no-JS path cannot learn — it PICKS: every legend row
// carries a real <form> whose <select name="key"> offers the whole key
// vocabulary, and one "bind by name" panel does the same with a function
// picker beside it. Both POST form-encoded bodies and take the 303 back to
// /map?slot=N&flash=… , exactly like the status page's session forms.
//
// Dogfood ledger #17 — and the reason these tables live HERE, in the
// compile-time twin, rather than beside the markup that uses them:
// `...CONST.map((x) => h(…x.field…))` is expanded by the compiler AT BUILD
// TIME into static markup, which is what makes a 122-option <select>
// affordable on all 25 legend rows (the item-body seam offers no nested
// list, and 122 hand-written literals per select is not a thing anyone
// maintains). But `extractFileConstants` reads the ROOT *Page file only —
// the same blind spot as #9's signal defaults — and it reads plain top-level
// `const` declarations, so `export const` would be invisible to it. Hence:
// declare bare here, `export { … }` at the bottom, and let MapIsland.ts
// import them back. Unlike the signals this is SINGLE-SOURCED; the import
// cycle it creates (MapPage → MapIsland → MapPage) is inert, because nothing
// touches these arrays until MapIsland() is called.
//
// `k` is the CONTRACT spelling — `Key::name()` in crates/ksx-core/src/key.rs,
// which is what a preset file stores and what `ksx map --key` parses, and it
// is what the option DISPLAYS too (see KeyOpt). The `<optgroup>` labels are
// the readability budget: they are static markup, so they can say anything.
// Excluded on purpose: `None` (the inert placeholder —
// clearing is the Clear button), `Unknown`, and the 13 mouse pseudo-keys (no
// capture path produces them yet). `every_bindable_key_name_is_offered` in
// render_map.rs pins this table against `Key::ALL` itself.

interface KeyOpt {
  /** The exact spelling that goes on the wire — and, because these render as
   *  `<option>` elements with NO value attribute, the text a user reads. HTML
   *  submits an option's trimmed text content when `value` is absent, which is
   *  exactly what is wanted here: what you pick is character-for-character
   *  what ends up in the preset file. (It is also the only shape that works:
   *  the compile-time spread substitutes member reads in CHILDREN, not in
   *  attribute values — see the ledger note above.) */
  k: string;
}

const KEYS_LETTER: KeyOpt[] = [
  { k: "A" },
  { k: "B" },
  { k: "C" },
  { k: "D" },
  { k: "E" },
  { k: "F" },
  { k: "G" },
  { k: "H" },
  { k: "I" },
  { k: "J" },
  { k: "K" },
  { k: "L" },
  { k: "M" },
  { k: "N" },
  { k: "O" },
  { k: "P" },
  { k: "Q" },
  { k: "R" },
  { k: "S" },
  { k: "T" },
  { k: "U" },
  { k: "V" },
  { k: "W" },
  { k: "X" },
  { k: "Y" },
  { k: "Z" },
];

const KEYS_DIGIT: KeyOpt[] = [
  { k: "One" },
  { k: "Two" },
  { k: "Three" },
  { k: "Four" },
  { k: "Five" },
  { k: "Six" },
  { k: "Seven" },
  { k: "Eight" },
  { k: "Nine" },
  { k: "Zero" },
];

const KEYS_FN: KeyOpt[] = [
  { k: "F1" },
  { k: "F2" },
  { k: "F3" },
  { k: "F4" },
  { k: "F5" },
  { k: "F6" },
  { k: "F7" },
  { k: "F8" },
  { k: "F9" },
  { k: "F10" },
  { k: "F11" },
  { k: "F12" },
];

const KEYS_NUMPAD: KeyOpt[] = [
  { k: "Numpad0" },
  { k: "Numpad1" },
  { k: "Numpad2" },
  { k: "Numpad3" },
  { k: "Numpad4" },
  { k: "Numpad5" },
  { k: "Numpad6" },
  { k: "Numpad7" },
  { k: "Numpad8" },
  { k: "Numpad9" },
  { k: "NumpadPlus" },
  { k: "NumpadMinus" },
  { k: "NumpadAsterisk" },
  { k: "NumpadDivide" },
  { k: "NumpadEnter" },
  { k: "NumpadDelete" },
  { k: "NumLock" },
];

const KEYS_ARROW: KeyOpt[] = [
  { k: "Up" },
  { k: "Down" },
  { k: "Left" },
  { k: "Right" },
];

const KEYS_NAV: KeyOpt[] = [
  { k: "Home" },
  { k: "End" },
  { k: "PageUp" },
  { k: "PageDown" },
  { k: "Insert" },
  { k: "Delete" },
];

const KEYS_EDIT: KeyOpt[] = [
  { k: "Escape" },
  { k: "Tab" },
  { k: "Enter" },
  { k: "Space" },
  { k: "Backspace" },
];

const KEYS_MOD: KeyOpt[] = [
  { k: "LeftShift" },
  { k: "RightShift" },
  { k: "ShiftModifier" },
  { k: "LeftControl" },
  { k: "RightControl" },
  { k: "LeftAlt" },
  { k: "RightAlt" },
  { k: "LeftWindows" },
  { k: "RightWindows" },
  { k: "CapsLock" },
  { k: "Menu" },
];

const KEYS_SYMBOL: KeyOpt[] = [
  { k: "DashUnderscore" },
  { k: "PlusEquals" },
  { k: "OpenBracketBrace" },
  { k: "CloseBracketBrace" },
  { k: "SemicolonColon" },
  { k: "SingleDoubleQuote" },
  { k: "Tilde" },
  { k: "BackslashPipe" },
  { k: "LeftBackslashPipe" },
  { k: "CommaLeftArrow" },
  { k: "PeriodRightArrow" },
  { k: "ForwardSlashQuestionMark" },
];

const KEYS_MEDIA: KeyOpt[] = [
  { k: "MediaPreviousTrack" },
  { k: "MediaPlayPause" },
  { k: "MediaNextTrack" },
  { k: "VolumeUp" },
  { k: "VolumeDown" },
  { k: "VolumeMute" },
  { k: "PrintScreen" },
  { k: "ScrollLock" },
  { k: "Break" },
  { k: "Pause" },
];

const KEYS_OEM: KeyOpt[] = [
  { k: "Oem0" },
  { k: "Oem2" },
  { k: "Oem3" },
  { k: "Oem4" },
  { k: "Oem5" },
  { k: "Oem6" },
  { k: "Oem7" },
  { k: "Oem13" },
  { k: "Oem16" },
];

/** The 25 mappable functions, as the preset vocabulary spells them (the same
 *  strings `ksx map --function` takes). Persona-neutral on purpose: this is
 *  the no-JS panel's picker, and the canonical name is what the CLI, the
 *  file and the daemon all agree on — the legend rows beside it show what the
 *  same control is CALLED on this pad. */
const FUNCTIONS: KeyOpt[] = [
  { k: "A" },
  { k: "B" },
  { k: "X" },
  { k: "Y" },
  { k: "lb" },
  { k: "rb" },
  { k: "lt" },
  { k: "rt" },
  { k: "back" },
  { k: "start" },
  { k: "guide" },
  { k: "lthumb" },
  { k: "rthumb" },
  { k: "dpad.up" },
  { k: "dpad.down" },
  { k: "dpad.left" },
  { k: "dpad.right" },
  { k: "lx.min" },
  { k: "lx.max" },
  { k: "ly.min" },
  { k: "ly.max" },
  { k: "rx.min" },
  { k: "rx.max" },
  { k: "ry.min" },
  { k: "ry.max" },
];

export {
  KEYS_LETTER,
  KEYS_DIGIT,
  KEYS_FN,
  KEYS_NUMPAD,
  KEYS_ARROW,
  KEYS_NAV,
  KEYS_EDIT,
  KEYS_MOD,
  KEYS_SYMBOL,
  KEYS_MEDIA,
  KEYS_OEM,
  FUNCTIONS,
};

export function MapPage() {
  return MapIsland();
}
