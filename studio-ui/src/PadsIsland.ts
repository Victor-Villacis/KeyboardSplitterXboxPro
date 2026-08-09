import { h, createSignal, createList, createShow } from "@getforma/core";

// The island: the whole /pads screen, live.
//
// Same two halves as StatusIsland.ts — the signal declarations below ARE the
// FMIR slot table, and the same signals are rewritten by the 2 s poller in
// pads.ts. Read that file's header for the protocol; this comment is only
// about what is different here.
//
// **This page decides nothing and words nothing.** Every sentence it shows
// arrives composed from `MachineSource::pads_view` — the summary, the devnode
// line, the owner line, the elevation warning, the confirm lead, the XInput
// ceiling paragraph, the prune plan, and the label on every single
// `<option>`. That is deliberate and it is the point of the page: "eight pads,
// four of which no game can read" is a fact about ksx's own limits, and a web
// page that worked it out for itself would be a second answer to a question
// the CLI already answers (docs/SURFACES.md §1). `applyPads` below is a
// copier, not a deriver, and it owns no wording at all.
//
// Four sentences used to live here AND in render_pads.rs, in two languages,
// with only the Rust half tested: the server painted one and the 2 s poller
// overwrote it with the other. They were not neutral formatting either — one
// asserted "this prune WILL be refused" and another "Every pad listed here
// goes, at once". They are `PadsView` fields now.
//
// Compiler constraints honored below (see render.rs):
// - dynamic text/attrs must be bare `() => signalName()` calls;
// - list sources are bare `() => listSignal()` calls;
// - list item bodies may only use direct member reads (`o.label`);
// - createShow conditions must be bare `() => signalName()` too;
// - createShows are SIBLINGS, never nested: every combined condition
//   (`restart && !confirm`) is computed in applyPads and gets its own signal.

// ── Wire types: serde field names from crates/ksx-api/src/machine.rs ────────

export interface VirtualPadRow {
  instance_id: string;
  hardware_id: string;
  persona: string;
  xinput: boolean;
}

export interface PrunePlanView {
  /** `nothing` | `no-bus` | `session-running` | `restart`. */
  kind: string;
  count: number;
  command: string | null;
  detail: string;
}

export interface SpawnOption {
  value: string;
  label: string;
}

/** The first option in each list is the default — that is what a `<select>`
 *  does, so the backend's ordering IS the default. */
export interface SpawnOffer {
  counts: SpawnOption[];
  personas: SpawnOption[];
  holds: SpawnOption[];
  note: string;
  refused: string | null;
}

export interface PadsView {
  generated_at: string;
  summary: string;
  bus_instance_id: string | null;
  bus_line: string;
  pads: VirtualPadRow[];
  owners: string[];
  owners_line: string;
  session_running: boolean;
  xinput_ceiling: number;
  /** How many XInput slots hold a pad — `null` means the reading FAILED, and
   *  that is not the same as zero. Rendered through `xinput_line`; nothing
   *  here does arithmetic on it. */
  xinput_in_use: number | null;
  xinput_line: string;
  elevated: boolean | null;
  elevation_line: string;
  confirm_line: string;
  /** The failed-read banner's heading — composed by `PadsView::unreadable`,
   *  empty on a read that answered. The h2 renders it verbatim; this file
   *  owns no copy of the sentence. */
  unreadable_heading: string;
  prune: PrunePlanView;
  spawn: SpawnOffer;
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

/** What GET /api/pads serves and what the island props carry — one shape
 *  (`PadsPayload` in snapshot.rs; the parity is unit-tested in render_pads.rs). */
export interface PadsPayload {
  pads: PadsView;
  session: SessionView;
  confirm: boolean;
  unavailable: string | null;
  flash: string | null;
}

interface PadTile {
  player: string;
  persona: string;
  instance: string;
  /** Vendored controller art for this persona (render.rs `art_for`). */
  art: string;
}

interface GhostTile {
  slot: string;
}

interface PruneRow {
  instance: string;
  persona: string;
}

/** Mirror of render.rs `PAD_TILE_FLOOR`: a 4-slot XInput cabinet at rest still
 *  LOOKS like a 4-slot cabinet. */
const PAD_TILE_FLOOR = 4;

// ── The live state store (module-level: one island, page lifetime) ─────────

const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");
const [padsSummary, setPadsSummary] = createSignal("not collected");
const [busLine, setBusLine] = createSignal("not collected");
const [ownersLine, setOwnersLine] = createSignal("not collected");
const [xinputLine, setXinputLine] = createSignal("not collected");
const [sessionLine, setSessionLine] = createSignal("not collected");
const [pruneDetail, setPruneDetail] = createSignal("not collected");
const [pruneCommand, setPruneCommand] = createSignal("");
const [elevationLine, setElevationLine] = createSignal("not collected");
const [spawnNote, setSpawnNote] = createSignal("not collected");
const [spawnRefusal, setSpawnRefusal] = createSignal("");
const [confirmLine, setConfirmLine] = createSignal("");
const [unreadableHeading, setUnreadableHeading] = createSignal("");
const [unavailableLine, setUnavailableLine] = createSignal("");
const [flashLine, setFlashLine] = createSignal("");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [unavailable, setUnavailable] = createSignal(false);
const [busRead, setBusRead] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [canSpawn, setCanSpawn] = createSignal(false);
const [spawnBlocked, setSpawnBlocked] = createSignal(false);
const [pruneIdle, setPruneIdle] = createSignal(false);
const [pruneArm, setPruneArm] = createSignal(false);
const [pruneArmed, setPruneArmed] = createSignal(false);
const [notElevated, setNotElevated] = createSignal(false);
const [hasCommand, setHasCommand] = createSignal(false);

const [padTiles, setPadTiles] = createSignal<PadTile[]>([]);
const [ghostTiles, setGhostTiles] = createSignal<GhostTile[]>([]);
const [countOptions, setCountOptions] = createSignal<SpawnOption[]>([]);
const [personaOptions, setPersonaOptions] = createSignal<SpawnOption[]>([]);
const [holdOptions, setHoldOptions] = createSignal<SpawnOption[]>([]);
const [pruneRows, setPruneRows] = createSignal<PruneRow[]>([]);

// ── Applying a payload ──────────────────────────────────────────────────────

/** Whether the destructive panel is armed, kept by the CLIENT.
 *
 *  `/api/pads` answers `confirm: false` on every poll, always and on purpose —
 *  a 2 s timer is not a user saying yes. Reading the panel's visibility
 *  straight off the payload would therefore close it two seconds after the
 *  user opened it, which is the sort of thing that gets clicked twice.
 *
 *  So the query drives the FIRST paint and the client keeps the choice
 *  afterwards — the same shape as the mapper's slot selection (`MapPayload`
 *  `selected`). Nothing here can arm it: `applyPads` only ever latches it TRUE
 *  from a payload that already said so, and `disarm` is the one way back. */
let armed = false;

/** Forget the arming. Called when the action has happened (pads.ts) — the
 *  "Cancel" link needs no help, it navigates to a clean `/pads`. */
export function disarm(): void {
  armed = false;
}

/** Write one /api/pads payload into every signal (flash excluded — flash is
 *  one-shot action feedback, owned by `applyFlash`). Safe to call before
 *  adoption AND per poll. */
export function applyPads(p: PadsPayload): void {
  const v = p.pads;
  const session = p.session;

  setGeneratedAt(v.generated_at);
  setPadsSummary(v.summary);
  setBusLine(v.bus_line);
  setOwnersLine(v.owners_line);
  setXinputLine(v.xinput_line);
  setSessionLine(session.line);
  setPruneDetail(v.prune.detail);
  setPruneCommand(v.prune.command ?? "");
  setElevationLine(v.elevation_line);
  setSpawnNote(v.spawn.note);
  setSpawnRefusal(v.spawn.refused ?? "");
  setConfirmLine(v.confirm_line);
  setUnreadableHeading(v.unreadable_heading ?? "");
  setUnavailableLine(p.unavailable ?? "");

  if (p.confirm) armed = true;
  const readable = p.unavailable === null || p.unavailable === undefined;
  const restart = v.prune.kind === "restart";
  setPillRunning(session.reachable && session.running);
  setPillIdle(session.reachable && !session.running);
  setPillDown(!session.reachable);
  setUnavailable(!readable);
  // The ceiling card's standing paragraph is a claim about this machine's
  // spawn menu, so it renders only when the read HAPPENED — under a failed
  // read it would contradict the xinput_line two lines above it, which is
  // busy saying ksx cannot count the slots.
  setBusRead(readable);
  setCanSpawn(v.spawn.refused === null || v.spawn.refused === undefined);
  setSpawnBlocked(v.spawn.refused !== null && v.spawn.refused !== undefined);
  // "Nothing for this panel to do right now" is an assertion of absence, so it
  // is gated on the read having HAPPENED. Mirrors show_values() in
  // render_pads.rs, which the slot-layout test pins name for name.
  setPruneIdle(!restart && readable);
  setPruneArm(restart && !armed);
  setPruneArmed(restart && armed);
  setNotElevated(restart && v.elevated === false);
  setHasCommand(restart && (v.prune.command ?? "") !== "");

  setCountOptions(v.spawn.counts);
  setPersonaOptions(v.spawn.personas);
  setHoldOptions(v.spawn.holds);
  setPruneRows(
    v.pads.map((pad) => ({ instance: pad.instance_id, persona: pad.persona })),
  );
  setPadTiles(
    v.pads.map((pad, i) => ({
      player: `P${i + 1}`,
      persona: pad.persona,
      instance: pad.instance_id,
      // Mirrors render.rs art_for(): PlayStation-ish personas get the DS4 art,
      // everything else the Xbox pad.
      art: /playstation|ds4|ps4/i.test(pad.persona)
        ? "/_assets/pad-ds4.svg"
        : "/_assets/pad-xbox.svg",
    })),
  );
  // Ghost tiles draw "this cabinet has four slots and they are empty", which
  // is a CLAIM. A read that failed shows none of them; the banner says what
  // actually happened instead.
  const ghosts: GhostTile[] = [];
  if (readable) {
    for (let i = v.pads.length; i < PAD_TILE_FLOOR; i++) {
      ghosts.push({ slot: `P${i + 1}` });
    }
  }
  setGhostTiles(ghosts);
}

/** The studio server itself stopped answering /api/pads. Say so, disable both
 *  verbs, and keep the last-known list visible — its timestamp stops
 *  advancing, which is the honest tell. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setCanSpawn(false);
  setSpawnBlocked(true);
  setSpawnRefusal("ksx Studio is not answering — nothing can be spawned until it does");
  setPruneArm(false);
  setPruneArmed(false);
  // The prune plan on screen is the last one that was READ, and with all three
  // prune panels hidden nothing on the card would say so — it would just sit
  // there looking current. This is the only wording this file owns, and it has
  // no backend twin by definition: the backend is the thing not answering.
  setPruneDetail(
    "ksx Studio is not answering — the plan above is the last one it read, and nothing can be pruned until it answers again.",
  );
  setPruneIdle(false);
  setNotElevated(false);
  setHasCommand(false);
}

/** One-shot action feedback (POST outcome or the seed's ?flash= value). */
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

export function PadsIsland() {
  return h(
    "div",
    { class: "studio" },
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
      // ── The read failed. An empty pad list would read as "your bus is
      // clean", which is the one thing it must never say by accident. ──────
      createShow(
        () => unavailable(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            // The heading is the PROVIDER's (`PadsView::unreadable_heading`),
            // like every other sentence here — it was hardcoded once and that
            // was a second copy of a machine.rs string in another language.
            h("h2", null, () => unreadableHeading()),
            h("p", { class: "alarmlead" }, () => unavailableLine()),
            h(
              "p",
              { class: "alarmlead" },
              "Nothing below is a reading of this machine. Run ",
              h("code", { class: "mono copyable" }, "ksx doctor"),
              " to find out why.",
            ),
          ),
      ),
      createShow(
        () => flashOk(),
        () => h("p", { class: "flash flash-ok" }, () => flashLine()),
      ),
      createShow(
        () => flashError(),
        () => h("p", { class: "flash flash-err" }, () => flashLine()),
      ),
      // ── ON THE BUS: what is actually plugged, right now ─────────────────
      h(
        "section",
        { class: "card wide padcard" },
        h("h2", null, "On the bus"),
        h("p", { class: "cardline" }, () => padsSummary()),
        h(
          "div",
          { class: "padgrid" },
          // KEY EVERY FIELD THE ROW RENDERS. forma reconciles by key and does
          // NOT patch a row whose key matched (`updateOnItemChange` defaults to
          // "none" — "reused rows keep their DOM"), so any member missing from
          // the key freezes at its first paint. render_pads.rs's
          // `every_list_row_reconciles_on_every_field_it_renders` reads this
          // file and fails on one that is.
          createList(
            () => padTiles(),
            (p) => p.player + "|" + p.persona + "|" + p.instance + "|" + p.art,
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
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "ViGEmBus devnode"),
          h("span", { class: "dvalue mono" }, () => busLine()),
        ),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Held by"),
          h("span", { class: "dvalue" }, () => ownersLine()),
        ),
        h(
          "div",
          { class: "drow" },
          h("span", { class: "dname" }, "Session"),
          h("span", { class: "dvalue" }, () => sessionLine()),
        ),
      ),
      // ── THE CEILING: the sentence that has to arrive before the click ───
      h(
        "section",
        { class: "card warnbox" },
        h("h2", null, "The XInput ceiling"),
        h("p", { class: "warn" }, () => xinputLine()),
        // Gated on the read having HAPPENED: this paragraph claims the counts
        // below carry labels and that four is the number games see, and under
        // a failed read the spawn offer is refused (no counts exist) while
        // the xinput_line above says ksx cannot count the slots. A standing
        // paragraph that contradicts the banner is the banner losing.
        createShow(
          () => busRead(),
          () =>
            h(
              "p",
              { class: "cardline" },
              "This is why every count below carries its own label. ksx will plug ",
              "whatever you ask for — it does not refuse a fifth Xbox pad — and ",
              "Windows will still only ever hand four of them to a game. ",
              "PlayStation pads are the way past it: they are plain HID, so eight ",
              "players is eight readable pads.",
            ),
        ),
      ),
      // ── SPAWN: the pad test, bounded ────────────────────────────────────
      h(
        "section",
        { class: "card" },
        h("h2", null, "Spawn test pads"),
        h("p", { class: "cardline" }, () => spawnNote()),
        createShow(
          () => canSpawn(),
          () =>
            h(
              "form",
              { class: "pactrow", method: "post", action: "/pads/spawn" },
              h(
                "label",
                { class: "bindlabel", for: "count" },
                "how many",
                // These three option lists key on value AND label (the key
                // rule above), so a relabelled option is a REBUILT option —
                // and a rebuilt <option> list resets its <select> to the
                // first entry. pads.ts captures the user's choice before
                // every applyPads and restores it after, so a 2 s reading
                // that moved does not discard what they picked
                // (render_pads.rs pins the pairing).
                h(
                  "select",
                  { id: "count", name: "count" },
                  createList(
                    () => countOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h(
                "label",
                { class: "bindlabel", for: "persona" },
                "as what",
                h(
                  "select",
                  { id: "persona", name: "persona" },
                  createList(
                    () => personaOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h(
                "label",
                { class: "bindlabel", for: "hold_secs" },
                "for how long",
                h(
                  "select",
                  { id: "hold_secs", name: "hold_secs" },
                  createList(
                    () => holdOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h("button", { class: "btn btn-primary", type: "submit" }, "Spawn"),
            ),
        ),
        createShow(
          () => spawnBlocked(),
          () =>
            h(
              "div",
              { class: "controls off" },
              h("button", { class: "btn", disabled: "" }, "Spawn"),
              h("p", { class: "warn" }, () => spawnRefusal()),
            ),
        ),
      ),
      // ── PRUNE: destructive, so it is two clicks and the second one lists
      // every pad it takes. ───────────────────────────────────────────────
      h(
        "section",
        { class: "card" },
        h("h2", null, "Prune stale pads"),
        h(
          "p",
          { class: "cardline" },
          "Pads outlive whatever made them: a splitter that was killed rather ",
          "than closed leaves its pads on the bus, and they accumulate. ",
          "Pruning restarts the ViGEmBus devnode, which drops every child pad ",
          "with it — there is no way to remove one and keep the others.",
        ),
        h("p", { class: "cardline mono" }, () => pruneDetail()),
        createShow(
          () => notElevated(),
          () => h("p", { class: "warn" }, () => elevationLine()),
        ),
        createShow(
          () => hasCommand(),
          () =>
            h(
              "p",
              { class: "clifall" },
              h("code", { class: "mono copyable" }, () => pruneCommand()),
            ),
        ),
        createShow(
          () => pruneIdle(),
          () =>
            h(
              "p",
              { class: "cardline" },
              "Nothing for this panel to do right now — the line above is ",
              "re-read from the bus every 2 s, so it will say when there is.",
            ),
        ),
        createShow(
          () => pruneArm(),
          () =>
            h(
              "p",
              { class: "pactrow" },
              h(
                "a",
                { class: "btn btn-danger-ghost", href: "/pads?confirm=1" },
                "Prune…",
              ),
            ),
        ),
        createShow(
          () => pruneArmed(),
          () =>
            h(
              "div",
              { class: "card alarm" },
              h("h2", null, "Remove these pads?"),
              h("p", { class: "alarmlead" }, () => confirmLine()),
              h(
                "ul",
                { class: "plist" },
                createList(
                  () => pruneRows(),
                  (r) => r.instance + "|" + r.persona,
                  (r) =>
                    h(
                      "li",
                      null,
                      h(
                        "div",
                        { class: "pmeta" },
                        h("span", { class: "ptitle" }, r.persona),
                        h("span", { class: "pdetail mono" }, r.instance),
                      ),
                    ),
                ),
              ),
              h(
                "div",
                { class: "pactrow" },
                h(
                  "form",
                  { method: "post", action: "/pads/prune" },
                  h("input", { type: "hidden", name: "confirm", value: "yes" }),
                  h(
                    "button",
                    { class: "btn btn-danger", type: "submit" },
                    "Yes, restart the bus",
                  ),
                ),
                h("a", { class: "btn btn-ghost", href: "/pads" }, "Cancel"),
              ),
            ),
        ),
      ),
    ),
    h(
      "footer",
      null,
      h(
        "p",
        null,
        "The bus is re-read every 2 s in place. Spawning plugs real pads and ",
        "unplugs them when the hold expires; pruning restarts a driver device ",
        "and needs an administrator token. Generated ",
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
