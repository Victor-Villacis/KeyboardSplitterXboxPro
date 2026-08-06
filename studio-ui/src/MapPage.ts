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
  // v14, the preset surface's identity block (twins in MapIsland.ts).
  const [presetLine] = createSignal("(no preset)");
  const [presetPath] = createSignal("(unknown)");
  const [backupFact] = createSignal("none yet — the first restore writes one");
  // v9: the selected slot number, as the hidden field of every no-JS form
  // that lives outside the legend list (preset actions, bind-by-name).
  const [slotNum] = createSignal("1");
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
  // v11, the MACRO EDITOR (the piano roll). Thirteen scalars and not one new
  // createShow: every state it has is a class string on an element that is
  // always in the DOM, so MAP_SHOW_ORDER does not move (ledger #4/#14 — shows
  // are positional, and one inserted in the middle silently shifts every panel
  // after it). Defaults are the honest "nothing read yet" values.
  const [macroHead] = createSignal("no macro loaded yet");
  const [macroRuleLine] = createSignal("");
  // v16: the direction RING, and what ticking a diagonal writes. SSR'd like
  // every other explainer — a page with no JavaScript cannot tick a cell, but
  // it can read what the columns mean and hand-write the pair.
  const [macroRingLine] = createSignal("");
  const [macroPolicyLine] = createSignal("on release: finish · retrigger: ignore · interrupt: none");
  const [macroNote] = createSignal("");
  const [macroTriggerLine] = createSignal("no trigger key yet — nothing starts this macro");
  // v12: never a made-up macro name. The old "my-macro" default existed only
  // in the browser, so its trigger could not be bound.
  const [macroFnName] = createSignal("");
  const [macroName] = createSignal("");
  const [macroCliLine] = createSignal(
    "ksx map --preset <NAME> --function macro.<NAME> --key <KEY>",
  );
  const [macroToml] = createSignal("");
  const [macroCardCls] = createSignal("card macrocard off");
  const [macroGridCls] = createSignal("macgrid empty");
  const [macroDirtyLine] = createSignal("");
  const [macroStepLine] = createSignal("click a step's ⏱ to edit its duration");
  const [macroDurValue] = createSignal("50");
  // v15/FIX 2: the duration BOX's own class (a below-floor step is a fact
  // about the number in that field), and Save's inline question about short
  // steps — a class string plus its sentence, never a show (ledger #13/#14).
  // An SSR paint has asked nothing, so the bar renders off and empty.
  const [macroDurCls] = createSignal("macdurin");
  const [macroConfirmCls] = createSignal("macconfirm off");
  const [macroConfirmLine] = createSignal("");
  // v15/FIX 1c: which mechanism the "common motions" buttons write, and why.
  const [macroMotionLine] = createSignal("");
  // v12's three: the Save button's look, the live frame arithmetic, and the
  // trigger block's inert class while the preset holds no macro.
  const [macroSaveCls] = createSignal("btn btn-mini macsave off");
  // v14: the per-macro switch (a class string plus its label — never a show),
  // and the slot-wide `macros = "off"` sentence. An SSR paint has read the
  // file, so both come from the payload; these are the honest defaults for a
  // page with no macro loaded and a slot that runs macros.
  const [macroEnableCls] = createSignal("btn btn-mini macen off dead");
  const [macroEnableLabel] = createSignal("Enabled");
  const [slotMacrosLine] = createSignal("");
  const [macroMathLine] = createSignal("");
  // v13: the repeat policy's own live math, and the rate box's value.
  const [macroTurboLine] = createSignal("");
  const [macroTurboValue] = createSignal("");
  // Client-only: the learn modal's auto-fire sentence (an SSR paint has no
  // modal open, so there is nothing to say about a control nobody picked).
  const [modalTurboLine] = createSignal("");
  const [macroTrigCls] = createSignal("mactrigger off");
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
