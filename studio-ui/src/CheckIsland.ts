import { h, createSignal, createList, createShow } from "@getforma/core";

// ── /check — THE BUTTON CHECK (docs/MAPPER-UX.md Build C) ──────────────────
//
// Press a panel key; every virtual control it drives lights, on EVERY slot at
// once. That is the diagnostic ("is this button wired?") and the product demo
// ("four pads glowing from one keystroke") in the same picture, and it is
// commandment 2's live echo — "every mapping screen is also a button-check
// screen" — given a screen of its own until the mapper can carry it.
//
// # Why chips and not four controller drawings
//
// The mapper already draws the pad, with 25 absolutely-positioned hit zones
// per persona. Four of those on one screen would be four sets of geometry to
// keep aligned, at a quarter size, on a phone held sideways behind a cabinet —
// and the responsive pass is a separate branch this one must not fight.
//
// So this page is the OTHER half of commandment 4: "render as summary, legend
// as TABLE". One flat grid of chips, each carrying its slot number, its
// customer-facing control label and the key that drives it. Big targets, no
// geometry, and the fan-out is if anything more legible: press G and four
// chips labelled P1 P2 P3 P4 light in the same row of your eye.
//
// # The hot path deliberately does not go through signals
//
// Every OTHER island rewrites its signals from a 2 s poller and lets the
// reconciler patch the DOM. This one is fed by an SSE stream at display rate,
// and rewriting a list signal of ~100 items sixty times a second would rebuild
// ~100 DOM nodes sixty times a second on the phone that is the point of the
// page.
//
// So the split is: SIGNALS own the structure (which slots, which controls,
// which keys — rewritten only when `/api/check` is re-read, every few
// seconds), and the live echo is a `classList` toggle on chips found by their
// `data-slot` / `data-control` attributes (check.ts `paint`). Nothing rewrites
// a list during an echo, so nothing can clobber it.
//
// Those two attributes are the whole contract between the two halves, and they
// are RAW VALUES — the slot number and the canonical control name, straight
// off the payload — rather than a composed id. That is on purpose: a composed
// `chip-1-dpad-up` would be a string spelled in Rust and again in TypeScript,
// which is exactly the class of drift render_check.rs's layout test exists to
// catch. There is no composed string here to keep in sync.
//
// Compiler constraints honored below (see render.rs):
// - dynamic text/attrs must be bare `() => signalName()` calls;
// - list sources are bare `() => listSignal()` calls;
// - list item bodies may only use direct member reads (`c.control`);
// - createShow conditions must be bare `() => signalName()` too;
// - createShows are SIBLINGS, never nested.

// ── Wire types ─────────────────────────────────────────────────────────────

/** One slot as the mapper snapshot describes it (`ksx_api::MapperSlot`). */
export interface MapperSlot {
  number: number;
  persona: string;
  persona_label: string;
  preset: string;
  keyboard: string;
  bindings: Record<string, string[]>;
}

export interface MapperSnapshot {
  generated_at: string;
  source: string;
  config_root: string;
  slots: MapperSlot[];
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

/** What `GET /api/check` serves and what this island's props carry — one
 *  shape (`CheckPayload` in snapshot.rs), parity pinned in render_check.rs. */
export interface CheckPayload {
  mapper: MapperSnapshot;
  session: SessionView;
  /** The provider's sentence for "this is what the feed is and where it comes
   *  from". Composed in Rust, printed here — this file words nothing. */
  feed_hint: string;
}

// ── The live stream's shapes (ksx_api::live) ───────────────────────────────

export interface KeyHit {
  key: string;
  device: string;
  alias: string;
  down: boolean;
}

export interface SlotLive {
  slot: number;
  /** Canonical control names held right now. */
  down: string[];
  /** ...and everything that went down since the last frame, even if it is
   *  already back up. This is the field that makes the check honest: a tap
   *  shorter than a frame is invisible in `down` and unmistakable here. */
  hit: string[];
  lt: number;
  rt: number;
  lx: number;
  ly: number;
  rx: number;
  ry: number;
}

export interface LiveFrame {
  running: boolean;
  slots: SlotLive[];
  keys: KeyHit[];
  dropped: number;
  off_panel: number;
}

export interface LiveEnvelope {
  frame: LiveFrame;
  unavailable?: string | null;
}

// ── Signals — this list IS the FMIR slot table ─────────────────────────────

/** One chip: one control of one slot. */
interface ControlChip {
  /** Slot NUMBER as a string — it is a DOM attribute value. */
  slot: string;
  /** `P1`, for the eye. */
  player: string;
  /** The canonical control name (`A`, `dpad.up`, `lt`) used only as the
   *  live-frame lookup key. */
  control: string;
  /** The same control in the words or symbols a player sees on the controller.
   *  `control` stays canonical for the live-feed lookup; this is display-only. */
  label: string;
  /** The keys that drive it, joined — or the provider's "unbound" tag. */
  keys: string;
}

/** One key the panel sent, for the big key column. */
interface KeyRow {
  key: string;
  alias: string;
  state: string;
}

interface EmptyPlayerRow {
  player: string;
  line: string;
  href: string;
  action: string;
}

const [generatedAt, setGeneratedAt] = createSignal("");
const [sourceLine, setSourceLine] = createSignal("");
const [emptyHeading, setEmptyHeading] = createSignal("");
const [emptyLine, setEmptyLine] = createSignal("");
const [emptyHref, setEmptyHref] = createSignal("/start");
const [emptyAction, setEmptyAction] = createSignal("Open Setup");
const [feedHint, setFeedHint] = createSignal("");
const [sessionLine, setSessionLine] = createSignal("");
/** The FEED's own state line — the daemon's `unavailable` sentence, or this
 *  file's two connection words. See `setFeedLine` in check.ts. */
const [feedLine, setFeedLine] = createSignal("");
/** "N frames were dropped…" — composed by check.ts from the frame's own
 *  counters, and shown rather than hidden. */
const [lossLine, setLossLine] = createSignal("");
const [offPanelLine, setOffPanelLine] = createSignal("");

const [chips, setChips] = createList<ControlChip>([]);
const [emptyPlayers, setEmptyPlayers] = createList<EmptyPlayerRow>([]);
const [keyRows, setKeyRows] = createList<KeyRow>([]);

const [live, setLive] = createSignal(false);
const [feedDown, setFeedDown] = createSignal(false);
const [hasSlots, setHasSlots] = createSignal(false);
const [noSlots, setNoSlots] = createSignal(false);
const [hasLoss, setHasLoss] = createSignal(false);
const [hasOffPanel, setHasOffPanel] = createSignal(false);
const [quiet, setQuiet] = createSignal(false);

// ── Appliers — copiers, never derivers ─────────────────────────────────────

interface EmptyState {
  heading: string;
  line: string;
  href: string;
  action: string;
}

/** Mapper snapshots deliberately use an unavailable sentinel rather than an
 *  empty healthy roster. Keep that state separate from a successful read with
 *  no controllers, and from a controller whose layout names no controls. */
function emptyState(mapper: MapperSnapshot): EmptyState | null {
  if (mapper.generated_at === "(unavailable)" || mapper.config_root === "(unavailable)") {
    return {
      heading: "Controls could not be checked",
      line:
        "Reopen ksx, then use Setup to confirm a controller and Controls to check its buttons. Nothing was changed.",
      href: "/start",
      action: "Open Setup",
    };
  }
  if (mapper.slots.length === 0) {
    return {
      heading: "No controller is ready to test",
      line: "Add a controller in Setup, then come back to test its buttons.",
      href: "/start",
      action: "Open Setup",
    };
  }
  if (mapper.slots.every((slot) => Object.keys(slot.bindings).length === 0)) {
    return {
      heading: "No controls are ready to test",
      line:
        "Open Controls and choose a ready-made layout or add button keys, then come back here.",
      href: "/map",
      action: "Open Controls",
    };
  }
  return null;
}

/** Canonical names stay in `data-control` for the live feed, but never have to
 *  be the label a customer reads. These are the same controller identities the
 *  Controls screen draws; unknown extension controls are still humanized. */
export function controlLabel(persona: string, control: string): string {
  const playstation = /playstation|ds4|ps4/i.test(persona);
  const standard: Record<string, string> = {
    A: playstation ? "✕" : "A",
    B: playstation ? "○" : "B",
    X: playstation ? "□" : "X",
    Y: playstation ? "△" : "Y",
    lt: playstation ? "L2" : "LT",
    lb: playstation ? "L1" : "LB",
    rb: playstation ? "R1" : "RB",
    rt: playstation ? "R2" : "RT",
    guide: playstation ? "PS" : "Guide",
    back: playstation ? "Share" : "View",
    start: playstation ? "Options" : "Menu",
    lthumb: "L3",
    rthumb: "R3",
    "ly.max": "Left stick ↑",
    "ly.min": "Left stick ↓",
    "lx.min": "Left stick ←",
    "lx.max": "Left stick →",
    "dpad.up": "D-pad ↑",
    "dpad.down": "D-pad ↓",
    "dpad.left": "D-pad ←",
    "dpad.right": "D-pad →",
    "ry.max": "Right stick ↑",
    "ry.min": "Right stick ↓",
    "rx.min": "Right stick ←",
    "rx.max": "Right stick →",
  };
  const known = standard[control];
  if (known) return known;
  if (control.startsWith("macro.")) {
    return `Button sequence “${control.slice("macro.".length)}”`;
  }
  const words = control.replace(/[._-]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "Other control";
}

/** Slot roster → chips. The CONTROL LIST is the backend's: it is the key set
 *  of `MapperSlot.bindings`, which is every function the preset names, unbound
 *  ones included (they arrive as an empty key list). A hardcoded roster here
 *  would be a second answer to "what controls does an Xbox pad have", and the
 *  cabinet's four-slot list is the standing reminder of what that costs. */
export function applyCheck(p: CheckPayload): void {
  setGeneratedAt(p.mapper.generated_at);
  setSourceLine("Press a keyboard or panel key and watch every controller action it drives.");
  setFeedHint(p.feed_hint);
  setSessionLine(
    p.session.running
      ? "Play is active."
      : p.session.reachable
        ? "Ready to test."
        : "Live testing needs ksx to be reopened.",
  );

  const empty = emptyState(p.mapper);
  setEmptyHeading(empty?.heading ?? "");
  setEmptyLine(empty?.line ?? "");
  setEmptyHref(empty?.href ?? "/start");
  setEmptyAction(empty?.action ?? "Open Setup");

  const rows: ControlChip[] = [];
  const missing: EmptyPlayerRow[] = [];
  for (const slot of p.mapper.slots) {
    if (Object.keys(slot.bindings).length === 0) {
      missing.push({
        player: `Player ${slot.number} has no controls yet`,
        line: "Open Controls and choose a ready-made layout or add button keys for this player.",
        href: `/map?slot=${slot.number}`,
        action: "Open Controls",
      });
    }
    for (const control of Object.keys(slot.bindings)) {
      const keys = slot.bindings[control] ?? [];
      rows.push({
        slot: String(slot.number),
        player: "P" + String(slot.number),
        control,
        label: controlLabel(slot.persona, control),
        keys: keys.length ? keys.join(" · ") : "unbound",
      });
    }
  }
  setChips(rows);
  setEmptyPlayers(missing);
  setHasSlots(empty === null && rows.length > 0);
  setNoSlots(empty !== null || rows.length === 0);
}

/** The feed's own state, in words. `down` is the visible half — a page that
 *  cannot reach the stream says so instead of showing a grid of dark chips,
 *  which is what a working feed looks like while nobody is pressing anything. */
export function applyFeedState(line: string, connected: boolean): void {
  setFeedLine(line);
  setLive(connected);
  setFeedDown(!connected);
}

/** Loss, REPORTED. Both counters come off the frame; neither is derived here
 *  and neither is swallowed. */
export function applyCounters(lossText: string, offPanelText: string): void {
  setLossLine(lossText);
  setHasLoss(lossText !== "");
  setOffPanelLine(offPanelText);
  setHasOffPanel(offPanelText !== "");
}

/** The key column. Empty means nothing has arrived yet, which the page says
 *  in words rather than leaving a blank strip. */
export function applyKeys(rows: KeyRow[]): void {
  setKeyRows(rows);
  setQuiet(rows.length === 0);
}

export function CheckIsland() {
  return h(
    "main",
    { class: "wrap check" },
    h(
      "nav",
      { class: "topnav", "aria-label": "screens" },
      h("a", { class: "navlink", href: "/start" }, "Setup"),
      h("a", { class: "navlink", href: "/map" }, "Controls"),
      h(
        "a",
        { class: "navlink on", href: "/check", "aria-current": "page" },
        "Test",
      ),
    ),
    h(
      "header",
      { class: "head" },
      h("h1", null, "Button check"),
      h("p", { class: "sub" }, () => sourceLine()),
      h("p", { class: "product-hidden" }, () => generatedAt()),
    ),

    // **The no-JS truth, first and unmissable.** This whole page is a live
    // echo, and a live echo is the one thing a document cannot do: with
    // scripting off there is no EventSource, so there are no frames, so
    // nothing can ever light. Saying so beats rendering a grid that looks
    // exactly like a working check on a panel nobody is touching — the
    // project's signature bug, inverted.
    //
    // The roster below it still renders and is still worth having: it is the
    // whole binding table, server-side, which answers "what SHOULD this key
    // do" even when it cannot answer "did it".
    h(
      "noscript",
      null,
      h(
        "div",
        { class: "alarm" },
        h("h2", null, "Live echo needs JavaScript"),
        h(
          "p",
          { class: "alarmlead" },
          "Live testing needs scripting switched on. Reopen ksx in its normal app window; the saved control list below remains readable, but it cannot light up here.",
        ),
      ),
    ),

    h(
      "section",
      { class: "card feedcard" },
      h("h2", null, "Live input"),
      h("p", { class: "dvalue" }, () => feedLine()),
      h("p", { class: "sub" }, () => sessionLine()),
      h("p", { class: "sub" }, () => feedHint()),
    ),

    createShow(
      () => hasLoss(),
      () => h("p", { class: "warnline" }, () => lossLine()),
    ),
    createShow(
      () => hasOffPanel(),
      () => h("p", { class: "warnline" }, () => offPanelLine()),
    ),
    createShow(
      () => feedDown(),
      () =>
        h(
          "p",
          { class: "sub" },
          "The controls below show the saved layout; they cannot light until live input is back.",
        ),
    ),

    h(
      "section",
      { class: "card keycard" },
      h("h2", null, "What the panel sent"),
      createShow(
        () => quiet(),
        () =>
          h(
            "p",
            { class: "sub" },
            "Nothing yet. Press a button on the panel.",
          ),
      ),
      h(
        "div",
        { class: "keystrip", id: "keystrip" },
        createList(
          () => keyRows(),
          (k) => k.key + "|" + k.alias + "|" + k.state,
          (k) =>
            h(
              "span",
              { class: "keyhit" },
              h("span", { class: "keyname" }, k.key),
              h("span", { class: "keyfrom" }, k.alias),
            ),
        ),
      ),
    ),

    createShow(
      () => noSlots(),
      () =>
        h(
          "section",
          { class: "card" },
          h("h2", null, () => emptyHeading()),
          h("p", { class: "sub" }, () => emptyLine()),
          h(
            "p",
            { class: "pactrow" },
            h("a", { class: "btn btn-primary", href: () => emptyHref() }, () => emptyAction()),
          ),
        ),
    ),
    createShow(
      () => hasSlots(),
      () =>
        h(
          "section",
          { class: "card chipcard" },
          h("h2", null, "Controller buttons"),
          h(
            "p",
            { class: "sub" },
            "Each controller button shows the key that controls it. A key shared by several players lights all of them at once.",
          ),
          createList(
            () => emptyPlayers(),
            (p) => p.player + "|" + p.href,
            (p) =>
              h(
                "div",
                { class: "warnbox" },
                h("h3", null, p.player),
                h("p", { class: "sub" }, p.line),
                h(
                  "p",
                  { class: "pactrow" },
                  h("a", { class: "btn btn-primary", href: p.href }, p.action),
                ),
              ),
          ),
          h(
            "div",
            { class: "chipgrid", id: "chipgrid" },
            createList(
              () => chips(),
              (c) => c.slot + "|" + c.control + "|" + c.label + "|" + c.keys,
              (c) =>
                h(
                  "div",
                  {
                    class: "chip",
                    "data-slot": c.slot,
                    "data-control": c.control,
                  },
                  h("span", { class: "chipslot" }, c.player),
                  h("span", { class: "chipname" }, c.label),
                  h("span", { class: "chipkeys mono" }, c.keys),
                ),
            ),
          ),
        ),
    ),

    createShow(
      () => live(),
      () =>
        h(
          "p",
          { class: "sub" },
          "Live. Even a very short button press flashes here.",
        ),
    ),
  );
}
