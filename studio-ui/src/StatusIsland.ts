import { h, createSignal, createList, createShow } from "@getforma/core";

// The island: the entire Studio screen, live.
//
// This file is BOTH halves of the islands protocol:
//
// - **Compile time** — `StatusPage.ts` returns `StatusIsland()`, and the
//   entry registers `StatusIsland` in `activateIslands`, so the compiler
//   inlines the h() tree below between ISLAND_START/ISLAND_END opcodes. The
//   signal DECLARATIONS below are the slot table too: compiler 0.3.1 walks
//   island component files for signal scopes, so the twin declarations
//   StatusPage.ts used to carry are gone (ledger #9, adopted 2026-08-06).
//   The Rust walker renders the island server-side with real data, stamps
//   `data-forma-island="0" data-forma-component="StatusIsland"` on the root
//   element — the client activation hook — and hangs `data-forma-props`
//   (every injected slot value) off the same element.
// - **Run time** — the module-level signals below are the page's one live
//   state store (single island, page lifetime). `applyStatus` seeds them from
//   the server's `__ksx-payload` block BEFORE adoption (the ledger-#5 rule:
//   hydration adopts whatever the signals hold, so they must hold the SSR
//   truth first), and the 2 s poller in `status.ts` keeps writing the SAME
//   signals afterwards.
//
// The derivations in `applyStatus` (summary sentences, pill booleans, pad
// tile numbering, ghost padding) deliberately MIRROR render.rs — the server
// derives them for first paint, the client re-derives them per poll from the
// same `/api/status` payload. render.rs unit tests pin the Rust side; keep
// both in sync when either changes.
//
// Compiler constraints honored below (see render.rs):
// - dynamic text/attrs must be bare `() => signalName()` calls — the slot is
//   named after the getter;
// - list sources are bare `() => listSignal()` calls — compiler 0.2.0 derives
//   the slot name from the binding (`list:padTiles:array`), which upgraded
//   the old positional `list:#N:array` seam;
// - list item bodies may only use direct member reads (`p.persona`);
// - createShow conditions must be bare `() => signalName()` too: 0.3.1 names
//   the Bool slot after that getter (`show:canStart`), which is what retired
//   render.rs's positional SHOW_ORDER (ledger #4);
// - every h() attribute must be a string literal — an identifier (the old
//   shared SIL_BODY const) silently compiles to an empty client slot, which
//   is why the silhouette `d` path is inlined verbatim in both tiles.

// ── Wire types: serde field names from crates/ksx-studio/src/{snapshot,control}.rs ──

export interface PadRow {
  persona: string;
  instance: string;
}

export interface ProfileRow {
  title: string;
  detail: string;
}

export interface StatusSnapshot {
  generated_at: string;
  vigem: string;
  interception: string;
  daemon_running: boolean;
  daemon_detail: string;
  autostart: string;
  pads: PadRow[];
  profiles: ProfileRow[];
  config_root: string;
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  /** games.toml profile the daemon is (or would be) pointed at — the --game
   *  flag the no-daemon banner's command needs on a profile-driven cabinet. */
  profile: string | null;
}

/** What GET /api/status serves and what the island props carry — one shape
 *  (`StatusPayload` in snapshot.rs; the parity is unit-tested there). */
export interface StatusPayload {
  snapshot: StatusSnapshot;
  session: SessionView;
  flash: string | null;
}

interface PadTile {
  player: string;
  persona: string;
  instance: string;
  /** Vendored controller art for this persona (render.rs `art_for`). */
  art: string;
  /** Per-slot jump into the mapper: `/map?slot=N`. */
  maphref: string;
}

interface GhostTile {
  slot: string;
}

/** Mirror of render.rs `PAD_TILE_FLOOR`: a 4-slot XInput cabinet at rest
 *  still LOOKS like a 4-slot cabinet. */
const PAD_TILE_FLOOR = 4;

// ── The live state store (module-level: one island, page lifetime) ─────────
// These declarations ARE the compile-time contract: the compiler reads them
// here and names the FMIR slots after the getters, so render.rs injects
// `vigemLine`, `show:canStart` and `list:padTiles:array` by those names.

const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");
const [vigemLine, setVigemLine] = createSignal("not collected");
const [interceptionLine, setInterceptionLine] = createSignal("not collected");
const [daemonYesNo, setDaemonYesNo] = createSignal("unknown");
const [daemonDetail, setDaemonDetail] = createSignal("not collected");
const [autostartLine, setAutostartLine] = createSignal("not collected");
const [padsSummary, setPadsSummary] = createSignal("not collected");
const [profilesSummary, setProfilesSummary] = createSignal("not collected");
const [configRoot, setConfigRoot] = createSignal("(unknown)");
const [sessionLine, setSessionLine] = createSignal("not collected");
const [flashLine, setFlashLine] = createSignal("");
const [daemonCmd, setDaemonCmd] = createSignal("ksx daemon");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [noDaemon, setNoDaemon] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [canStart, setCanStart] = createSignal(false);
const [canStop, setCanStop] = createSignal(false);
const [daemonDown, setDaemonDown] = createSignal(false);
const [vigemOk, setVigemOk] = createSignal(false);
const [vigemWarn, setVigemWarn] = createSignal(false);
const [icptBorrowed, setIcptBorrowed] = createSignal(false);
const [icptAbsent, setIcptAbsent] = createSignal(false);
const [autostartOn, setAutostartOn] = createSignal(false);
const [autostartOff, setAutostartOff] = createSignal(false);
const [rowsLive, setRowsLive] = createSignal(false);
const [rowsPlain, setRowsPlain] = createSignal(false);

const [profileOptions, setProfileOptions] = createSignal<ProfileRow[]>([]);
const [padTiles, setPadTiles] = createSignal<PadTile[]>([]);
const [ghostTiles, setGhostTiles] = createSignal<GhostTile[]>([]);
const [profileRows, setProfileRows] = createSignal<ProfileRow[]>([]);

// ── Derivations (mirror render.rs; pinned there by unit tests) ─────────────

function padsSummaryLine(count: number): string {
  if (count === 0) return "no virtual pads exposed by the bus";
  if (count === 1) return "1 virtual pad exposed by the bus:";
  return `${count} virtual pads exposed by the bus:`;
}

function profilesSummaryLine(count: number): string {
  if (count === 0) return "no profiles in games.toml";
  if (count === 1) return "1 profile in games.toml:";
  return `${count} profiles in games.toml:`;
}

/** Write one /api/status payload into every signal (flash excluded — flash
 *  is one-shot action feedback, owned by `applyFlash`). Safe to call before
 *  adoption AND per poll. */
export function applyStatus(p: StatusPayload): void {
  const snap = p.snapshot;
  const session = p.session;

  setGeneratedAt(snap.generated_at);
  setVigemLine(snap.vigem);
  setInterceptionLine(snap.interception);
  setDaemonYesNo(snap.daemon_running ? "yes" : "no");
  setDaemonDetail(snap.daemon_detail);
  setAutostartLine(snap.autostart);
  setPadsSummary(padsSummaryLine(snap.pads.length));
  setProfilesSummary(profilesSummaryLine(snap.profiles.length));
  setConfigRoot(snap.config_root);
  setSessionLine(session.line);
  setDaemonCmd(session.profile ? `ksx daemon --game "${session.profile}"` : "ksx daemon");

  const okVigem = snap.vigem.startsWith("installed — service running");
  const icptInstalled = snap.interception.startsWith("installed");
  const onAutostart = snap.autostart.startsWith("registered");
  const startable = session.reachable && !session.running;

  setPillRunning(session.reachable && session.running);
  setPillIdle(startable);
  setPillDown(!session.reachable);
  setNoDaemon(!session.reachable);
  setCanStart(startable);
  setCanStop(session.reachable && session.running);
  setDaemonDown(!session.reachable);
  setVigemOk(okVigem);
  setVigemWarn(!okVigem);
  setIcptBorrowed(icptInstalled);
  setIcptAbsent(!icptInstalled);
  setAutostartOn(onAutostart);
  setAutostartOff(!onAutostart);
  setRowsLive(startable);
  setRowsPlain(!startable);

  setProfileOptions(snap.profiles);
  setProfileRows(snap.profiles);
  setPadTiles(
    snap.pads.map((pad, i) => ({
      player: `P${i + 1}`,
      persona: pad.persona,
      instance: pad.instance,
      // Mirrors render.rs art_for(): PlayStation-ish personas get the DS4
      // art, everything else the Xbox pad.
      art: /playstation|ds4|ps4/i.test(pad.persona)
        ? "/_assets/pad-ds4.svg"
        : "/_assets/pad-xbox.svg",
      maphref: `/map?slot=${i + 1}`,
    })),
  );
  const ghosts: GhostTile[] = [];
  for (let i = snap.pads.length; i < PAD_TILE_FLOOR; i++) {
    ghosts.push({ slot: `P${i + 1}` });
  }
  setGhostTiles(ghosts);
}

/** The studio server itself stopped answering /api/status: say so, disable
 *  the controls, keep the last-known status cards visible (their timestamp
 *  stops advancing, which is the honest tell). */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setNoDaemon(true);
  setCanStart(false);
  setCanStop(false);
  setDaemonDown(true);
  setRowsLive(false);
  setRowsPlain(true);
}

/** One-shot action feedback (POST outcome or the seed's ?flash= value).
 *  Auto-clears after FLASH_MS — the live page never navigates, so nothing
 *  else would clear it (the v3 page relied on the meta refresh for that). */
const FLASH_MS = 5000;
let flashTimer: ReturnType<typeof setTimeout> | undefined;

export function applyFlash(flash: string | null | undefined): void {
  if (flashTimer !== undefined) {
    clearTimeout(flashTimer);
    flashTimer = undefined;
  }
  const line = (flash ?? "").trim();
  if (line === "") {
    setFlashLine("");
    setFlashOk(false);
    setFlashError(false);
    return;
  }
  const isError = line.startsWith("error");
  setFlashLine(line);
  setFlashOk(!isError);
  setFlashError(isError);
  flashTimer = setTimeout(() => applyFlash(null), FLASH_MS);
}

// ── The screen (the slot layout test pins its names) ───────────────────────

export function StatusIsland() {
  return h(
    "div",
    { class: "studio" },
    // ── App shell: brand, route nav, live state ─────────────────────────
    // The nav is a REAL two-item rail on both screens (v14): a wordmark and a
    // one-way "Mapper →" link read as a page that happens to have a sibling;
    // a rail with the current route marked reads as an application.
    h(
      "header",
      { class: "top" },
      h(
        "div",
        { class: "brand" },
        h("span", { class: "brand-ksx" }, "ksx"),
        h("span", { class: "brand-studio" }, "Studio"),
      ),
      h(
        "nav",
        { class: "topnav", "aria-label": "screens" },
        h("a", { class: "navlink", href: "/start" }, "Setup"),
        h("a", { class: "navlink", href: "/map" }, "Controls"),
        h("a", { class: "navlink", href: "/check" }, "Test"),
      ),
      createShow(
        () => pillRunning(),
        () => h("span", { class: "pill pill-run" }, "running"),
      ),
      createShow(
        () => pillIdle(),
        () => h("span", { class: "pill pill-idle" }, "idle"),
      ),
      createShow(
        () => pillDown(),
        () => h("span", { class: "pill pill-down" }, "no daemon"),
      ),
    ),
    h(
      "main",
      null,
      // ── FIX 1: the no-daemon banner, TOP of the page and identical in
      // wording to the mapper's. The session card below already renders its
      // controls disabled; this is what makes the state impossible to miss
      // before you have touched anything. ───────────────────────────────
      createShow(
        () => noDaemon(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            h(
              "h2",
              null,
              "No daemon — ksx Studio can see your config but cannot change anything.",
            ),
            h(
              "p",
              { class: "alarmlead" },
              "Everything below is a real reading of this machine. Nothing on this ",
              "page can start, stop or change anything until a daemon is running. ",
              "Two ways to start one:",
            ),
            h(
              "ol",
              { class: "alarmways" },
              h("li", null, "the ksx tray icon → Start emulation, or"),
              h(
                "li",
                null,
                "run this in a shell: ",
                h("code", { class: "mono copyable" }, () => daemonCmd()),
              ),
            ),
          ),
      ),
      // ── SESSION: the hero card ────────────────────────────────────────
      h(
        "section",
        { class: "card hero session" },
        h("h2", null, "Session"),
        h("p", { class: "state" }, () => sessionLine()),
        // Flash = feedback from the LAST action, visually distinct from
        // the state line above (which is the present-tense truth) and
        // always rendered under it. Cleared by applyFlash's timer.
        createShow(
          () => flashOk(),
          () => h("p", { class: "flash flash-ok" }, () => flashLine()),
        ),
        createShow(
          () => flashError(),
          () => h("p", { class: "flash flash-err" }, () => flashLine()),
        ),
        createShow(
          () => canStart(),
          () =>
            h(
              "form",
              { class: "controls", method: "post", action: "/session/start" },
              h("label", { for: "profile" }, "profile"),
              h(
                "select",
                { id: "profile", name: "profile" },
                h("option", { value: "" }, "(config default)"),
                createList(
                  () => profileOptions(),
                  (o) => o.title,
                  (o) => h("option", null, o.title),
                ),
              ),
              h("button", { class: "btn btn-primary", type: "submit" }, "Start"),
            ),
        ),
        createShow(
          () => canStop(),
          () =>
            h(
              "div",
              { class: "controls" },
              h(
                "form",
                { method: "post", action: "/session/stop" },
                h("button", { class: "btn btn-danger", type: "submit" }, "Stop"),
              ),
              h(
                "form",
                { method: "post", action: "/config/reload" },
                h("button", { class: "btn", type: "submit" }, "Reload config"),
              ),
            ),
        ),
        createShow(
          () => daemonDown(),
          () =>
            h(
              "div",
              { class: "controls off" },
              h(
                "select",
                { disabled: "" },
                h("option", null, "(profiles unavailable)"),
              ),
              h("button", { class: "btn", disabled: "" }, "Start"),
              h(
                "p",
                { class: "warn" },
                "controls disabled — no daemon control channel: ",
                "start the daemon (tray, or `ksx daemon`)",
              ),
            ),
        ),
      ),
      // ── VIRTUAL PADS: the signature card ──────────────────────────────
      h(
        "section",
        { class: "card wide padcard" },
        h("h2", null, "Virtual pads"),
        h("p", { class: "cardline" }, () => padsSummary()),
        // The deep link that makes /pads findable. A page reachable only from
        // the top nav is a page nobody finds, and "no virtual pads exposed by
        // the bus" is exactly the sentence someone wants to act on.
        h(
          "p",
          { class: "cardline" },
          h("a", { class: "maplink", href: "/pads" }, "Spawn or prune pads →"),
        ),
        h(
          "div",
          { class: "padgrid" },
          createList(
            () => padTiles(),
            // Composite key: any visible change (player number shifts when a
            // pad unplugs) re-renders the tile instead of leaving stale text
            // — reconcile keeps, it does not re-render, matching keys.
            (p) => p.player + "|" + p.persona + "|" + p.instance,
            // v5: the tile art is the REAL pad (Gamepad-Asset-Pack, MIT —
            // vendored, served from the embed), per persona via p.art; the
            // v3/v4 hand-drawn silhouette is gone. `Map` jumps to /map for
            // this slot.
            (p) =>
              h(
                "div",
                { class: "padtile live" },
                h("img", { class: "tileart", src: p.art, alt: p.persona }),
                h(
                  "div",
                  { class: "padmeta" },
                  h("span", { class: "player" }, p.player),
                  h("span", { class: "persona" }, p.persona),
                  h("a", { class: "maplink", href: p.maphref }, "Map"),
                ),
                h("div", { class: "instance" }, p.instance),
              ),
          ),
          createList(
            () => ghostTiles(),
            (g) => g.slot,
            (g) =>
              h(
                "div",
                { class: "padtile ghost" },
                h("img", {
                  class: "tileart",
                  src: "/_assets/pad-xbox.svg",
                  alt: "empty slot",
                }),
                h(
                  "div",
                  { class: "padmeta" },
                  h("span", { class: "player" }, g.slot),
                  h("span", { class: "persona" }, "empty"),
                ),
                h("div", { class: "instance" }, " "),
              ),
          ),
        ),
      ),
      // ── PROFILES: one row per games.toml entry, one click to start ────
      h(
        "section",
        { class: "card wide profilecard" },
        h("h2", null, "Profiles"),
        h(
          "p",
          { class: "cardline" },
          "Each profile is a games.toml entry: the program to launch and the ",
          "slots it hands out. Starting one is what puts pads on the bus.",
        ),
        h("p", { class: "cardline mono" }, () => profilesSummary()),
        createShow(
          () => rowsLive(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => profileRows(),
                (g) => g.title + "|" + g.detail,
                (g) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, g.title),
                      h("span", { class: "pdetail" }, g.detail),
                    ),
                    h(
                      "form",
                      { method: "post", action: "/session/start" },
                      h("input", { type: "hidden", name: "profile", value: g.title }),
                      h("button", { class: "btn btn-row", type: "submit" }, "Start"),
                    ),
                  ),
              ),
            ),
        ),
        createShow(
          () => rowsPlain(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => profileRows(),
                (g) => g.title + "|" + g.detail,
                (g) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, g.title),
                      h("span", { class: "pdetail" }, g.detail),
                    ),
                  ),
              ),
            ),
        ),
      ),
      // ── SYSTEM: the plumbing, LAST and quiet ──────────────────────────
      // v14: was two equal-weight cards in the middle of the page (an
      // Autostart card four fifths empty beside a Drivers one), which put the
      // least actionable facts on the screen at the same volume as the
      // session. One panel, key/value rows, tertiary surface, bottom of the
      // page. Show order inside is untouched — render.rs SHOW_ORDER moved the
      // whole block, not its contents.
      h(
        "section",
        { class: "card sysinfo" },
        h("h2", null, "System"),
        h(
          "p",
          { class: "cardline" },
          "What ksx is standing on. Nothing here is a control — it is what the ",
          "machine reports right now.",
        ),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "ViGEmBus"),
          createShow(
            () => vigemOk(),
            () => h("span", { class: "pill pill-ok" }, "OK"),
          ),
          createShow(
            () => vigemWarn(),
            () => h("span", { class: "pill pill-warn" }, "attention"),
          ),
        ),
        h("p", { class: "ddetail" }, () => vigemLine()),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Interception"),
          createShow(
            () => icptBorrowed(),
            () => h("span", { class: "pill pill-warn" }, "borrowed time"),
          ),
          createShow(
            () => icptAbsent(),
            () => h("span", { class: "pill pill-idle" }, "absent"),
          ),
        ),
        h("p", { class: "ddetail" }, () => interceptionLine()),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Autostart at logon"),
          createShow(
            () => autostartOn(),
            () => h("span", { class: "pill pill-ok" }, "on"),
          ),
          createShow(
            () => autostartOff(),
            () => h("span", { class: "pill pill-idle" }, "off"),
          ),
        ),
        h("p", { class: "ddetail" }, () => autostartLine()),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Daemon process"),
          h("span", { class: "dvalue" }, () => daemonYesNo()),
        ),
        h("p", { class: "ddetail" }, () => daemonDetail()),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Config root"),
          h("span", { class: "dvalue mono" }, () => configRoot()),
        ),
      ),
    ),
    // ── Footer: the plumbing facts, out of the body ───────────────────────
    h(
      "footer",
      null,
      h(
        "p",
        null,
        "Status re-read every 2 s in place; buttons go over the daemon pipe ",
        "(\\\\.\\pipe\\ksx-daemon). Without JavaScript the page auto-refreshes ",
        "every 5 s instead. Generated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Serving 127.0.0.1 only.",
      ),
      h(
        "p",
        null,
        "controller art: ",
        h(
          "a",
          { href: "https://github.com/AL2009man/Gamepad-Asset-Pack" },
          "Gamepad-Asset-Pack (MIT) by AL2009man",
        ),
      ),
    ),
  );
}
