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
// canonical control name and the key that drives it. Big targets, no
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
  /** The canonical control name (`A`, `dpad.up`, `lt`) — the vocabulary the
   *  preset file, the legend and the live frame all spell it in. */
  control: string;
  /** The keys that drive it, joined — or the provider's "unbound" tag. */
  keys: string;
}

/** One key the panel sent, for the big key column. */
interface KeyRow {
  key: string;
  alias: string;
  state: string;
}

const [generatedAt, setGeneratedAt] = createSignal("");
const [sourceLine, setSourceLine] = createSignal("");
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
const [keyRows, setKeyRows] = createList<KeyRow>([]);

const [live, setLive] = createSignal(false);
const [feedDown, setFeedDown] = createSignal(false);
const [hasSlots, setHasSlots] = createSignal(false);
const [noSlots, setNoSlots] = createSignal(false);
const [hasLoss, setHasLoss] = createSignal(false);
const [hasOffPanel, setHasOffPanel] = createSignal(false);
const [quiet, setQuiet] = createSignal(false);

// ── Appliers — copiers, never derivers ─────────────────────────────────────

/** Slot roster → chips. The CONTROL LIST is the backend's: it is the key set
 *  of `MapperSlot.bindings`, which is every function the preset names, unbound
 *  ones included (they arrive as an empty key list). A hardcoded roster here
 *  would be a second answer to "what controls does an Xbox pad have", and the
 *  cabinet's four-slot list is the standing reminder of what that costs. */
export function applyCheck(p: CheckPayload): void {
  setGeneratedAt(p.mapper.generated_at);
  setSourceLine(p.mapper.source);
  setFeedHint(p.feed_hint);
  setSessionLine(p.session.line);

  const rows: ControlChip[] = [];
  for (const slot of p.mapper.slots) {
    for (const control of Object.keys(slot.bindings)) {
      const keys = slot.bindings[control] ?? [];
      rows.push({
        slot: String(slot.number),
        player: "P" + String(slot.number),
        control,
        keys: keys.length ? keys.join(" · ") : "unbound",
      });
    }
  }
  setChips(rows);
  setHasSlots(rows.length > 0);
  setNoSlots(rows.length === 0);
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
      h("a", { class: "navlink", href: "/" }, "Status"),
      h("a", { class: "navlink", href: "/map" }, "Mapper"),
      h(
        "a",
        { class: "navlink on", href: "/check", "aria-current": "page" },
        "Check",
      ),
      h("a", { class: "navlink", href: "/pads" }, "Pads"),
      h("a", { class: "navlink", href: "/devices" }, "Devices"),
      h("a", { class: "navlink", href: "/profiles" }, "Profiles"),
      h("a", { class: "navlink", href: "/setup" }, "Setup"),
    ),
    h(
      "header",
      { class: "head" },
      h("h1", null, "Button check"),
      h("p", { class: "sub" }, () => sourceLine()),
      h("p", { class: "sub mono" }, () => generatedAt()),
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
          "This screen watches the daemon's input stream as it happens, which needs scripting switched on. Nothing below will light up. The binding table is still correct — it is read from disk on the server — so it still answers what each key SHOULD do; it cannot answer whether it did.",
        ),
        h(
          "p",
          { class: "alarmlead" },
          "Without scripting, `ksx monitor` in a terminal is the same check: one line per key as it arrives.",
        ),
      ),
    ),

    h(
      "section",
      { class: "card feedcard" },
      h("h2", null, "Feed"),
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
          "Chips below show the bindings on disk; they cannot light until the feed is back.",
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
          h("h2", null, "No slots to check"),
          h("p", { class: "sub" }, () => sourceLine()),
        ),
    ),
    createShow(
      () => hasSlots(),
      () =>
        h(
          "section",
          { class: "card chipcard" },
          h("h2", null, "Virtual controls"),
          h(
            "p",
            { class: "sub" },
            "One chip per control per slot. A key bound to several slots lights all of them at once — that is the fan-out, made visible.",
          ),
          h(
            "div",
            { class: "chipgrid", id: "chipgrid" },
            createList(
              () => chips(),
              (c) => c.slot + "|" + c.control + "|" + c.keys,
              (c) =>
                h(
                  "div",
                  {
                    class: "chip",
                    "data-slot": c.slot,
                    "data-control": c.control,
                  },
                  h("span", { class: "chipslot" }, c.player),
                  h("span", { class: "chipname" }, c.control),
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
          "Live. Frames arrive as they happen; a press shorter than a frame still flashes.",
        ),
    ),
  );
}
