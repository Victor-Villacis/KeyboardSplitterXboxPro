import { h, createSignal, createList, createShow } from "@getforma/core";

// The island: the whole /start screen — docs/FIRST-RUN.md moments 4 to 7.
//
// Same two halves as every other island: the signal declarations below ARE the
// FMIR slot table, and the same signals are rewritten by the poller in
// start.ts. Read PadsIsland.ts's header for the protocol; this comment is only
// about what is different here.
//
// **This page decides nothing and words nothing.** Every sentence arrives
// composed — the staging view's rosters and ceilings from `ksx-api`, the
// device rows from `DeviceScanView::read`, and the page's own lines from
// `StartLines` / `StartFlags` / `StartRows` in snapshot.rs. `applyStart` below
// is a copier. That is docs/SURFACES.md §1a, and this page has three specific
// reasons to obey it harder than most:
//
//   - MAX_SLOTS and MAX_XINPUT_SLOTS appear in its copy. A `16` typed here is
//     the exact bug the rule exists for (§1a records the Profiles page's).
//   - The persona list is a ROSTER with a can_plug flag per entry. Hardcoding
//     five names would keep offering `dualsense` after it starts plugging, or
//     keep offering it while it cannot.
//   - The split-or-freeze wording, the escape hatch and the per-session scope
//     are §3's own words about what the CAPTURE THREAD does. Paraphrasing the
//     first one is not a style slip: it is the only thing standing between a
//     frozen keyboard and a reboot.
//
// Compiler constraints honored below (see render.rs): dynamic text/attrs are
// bare `() => signalName()` calls, list sources are bare `() => listSignal()`,
// list item bodies use direct member reads only, createShow conditions are
// bare signal calls, and createShows are SIBLINGS — every combined condition
// is decided in Rust (StartFlags) and gets its own signal.

// ── Wire types: serde field names from crates/ksx-studio/src/snapshot.rs ────

export interface StartLines {
  device_line: string;
  device_detail: string;
  boards_line: string;
  controller_line: string;
  xinput_line: string;
  blocking_line: string;
  preset_line: string;
  mapper_line: string;
  ready_line: string;
  play_line: string;
  guide_line: string;
  stage_error: string;
  scan_error: string;
  presets_error: string;
}

export interface StartFlags {
  pill_running: boolean;
  pill_idle: boolean;
  pill_down: boolean;
  stage_down: boolean;
  scan_down: boolean;
  presets_down: boolean;
  has_device: boolean;
  has_boards: boolean;
  no_boards: boolean;
  has_other: boolean;
  has_notes: boolean;
  has_slots: boolean;
  can_add: boolean;
  slots_full: boolean;
  has_gaps: boolean;
  can_layout: boolean;
  blocking_answered: boolean;
  ready: boolean;
  not_ready: boolean;
  can_discard: boolean;
  session_live: boolean;
  flash_ok: boolean;
  flash_error: boolean;
}

export interface StartBoardRow {
  name: string;
  transport: string;
  backends: string;
  verdict: string;
  caveat: string;
  caveat_cls: string;
  cannot_type: string;
  cannot_type_cls: string;
  path: string;
  selector: string;
  alias: string;
  chosen_cls: string;
  button: string;
}

export interface StartOtherRow {
  name: string;
  transport: string;
  reason: string;
  backends: string;
}

export interface StartSlotRow {
  number: string;
  title: string;
  state: string;
  persona: string;
  xinput: string;
  preset: string;
  bindings: string;
}

export interface StartOptionRow {
  value: string;
  label: string;
}

export interface StartGapRow {
  label: string;
  gap: string;
  instead: string;
}

export interface StartLayoutRow {
  label: string;
  panel: string;
  players: string;
}

export interface StartBlockingRow {
  name: string;
  title: string;
  detail: string;
  chosen_cls: string;
  button: string;
}

export interface StartTextRow {
  text: string;
}

export interface StartRows {
  boards: StartBoardRow[];
  other: StartOtherRow[];
  notes: StartTextRow[];
  slots: StartSlotRow[];
  personas: StartOptionRow[];
  gaps: StartGapRow[];
  blocking: StartBlockingRow[];
  layouts: StartOptionRow[];
  layout_details: StartLayoutRow[];
  slot_numbers: StartOptionRow[];
}

/** The staged setup as ksx-api serves it. Only the fields this screen reads
 *  are named — the rest travel and are ignored, which is what keeps a new
 *  served field from breaking hydration. */
export interface StagedSetupView {
  reachable: boolean;
  next_slot: number | null;
  /** The preset name "Add a controller" posts. SERVED, because it becomes a
   *  file name — see snapshot.rs. */
  next_preset: string | null;
  escape_hatch: string;
  blocking_scope: string;
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

/** What GET /api/start serves and what the island props carry — one shape
 *  (`StartPayload` in snapshot.rs; parity unit-tested in render_start.rs). */
export interface StartPayload {
  staged: StagedSetupView;
  session: SessionView;
  flash: string | null;
  lines: StartLines;
  flags: StartFlags;
  rows: StartRows;
}

// ── The live state store (module-level: one island, page lifetime) ─────────

const [sessionLine, setSessionLine] = createSignal("not collected");
const [deviceLine, setDeviceLine] = createSignal("not collected");
const [deviceDetail, setDeviceDetail] = createSignal("");
const [boardsLine, setBoardsLine] = createSignal("not collected");
const [controllerLine, setControllerLine] = createSignal("not collected");
const [xinputLine, setXinputLine] = createSignal("not collected");
const [blockingLine, setBlockingLine] = createSignal("not collected");
const [presetLine, setPresetLine] = createSignal("not collected");
const [mapperLine, setMapperLine] = createSignal("not collected");
const [readyLine, setReadyLine] = createSignal("not collected");
const [playLine, setPlayLine] = createSignal("not collected");
const [guideLine, setGuideLine] = createSignal("not collected");
const [escapeLine, setEscapeLine] = createSignal("not collected");
const [scopeLine, setScopeLine] = createSignal("not collected");
const [stageError, setStageError] = createSignal("");
const [scanError, setScanError] = createSignal("");
const [presetsError, setPresetsError] = createSignal("");
const [nextPreset, setNextPreset] = createSignal("");
const [flashLine, setFlashLine] = createSignal("");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [stageDown, setStageDown] = createSignal(false);
const [scanDown, setScanDown] = createSignal(false);
const [presetsDown, setPresetsDown] = createSignal(false);
const [hasDevice, setHasDevice] = createSignal(false);
const [hasBoards, setHasBoards] = createSignal(false);
const [noBoards, setNoBoards] = createSignal(false);
const [hasOther, setHasOther] = createSignal(false);
const [hasNotes, setHasNotes] = createSignal(false);
const [hasSlots, setHasSlots] = createSignal(false);
const [canAdd, setCanAdd] = createSignal(false);
const [slotsFull, setSlotsFull] = createSignal(false);
const [hasGaps, setHasGaps] = createSignal(false);
const [canLayout, setCanLayout] = createSignal(false);
const [blockingAnswered, setBlockingAnswered] = createSignal(false);
const [ready, setReady] = createSignal(false);
const [notReady, setNotReady] = createSignal(false);
const [canDiscard, setCanDiscard] = createSignal(false);
const [sessionLive, setSessionLive] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);

const [boardRows, setBoardRows] = createSignal<StartBoardRow[]>([]);
const [otherRows, setOtherRows] = createSignal<StartOtherRow[]>([]);
const [noteRows, setNoteRows] = createSignal<StartTextRow[]>([]);
const [slotRows, setSlotRows] = createSignal<StartSlotRow[]>([]);
const [personaOptions, setPersonaOptions] = createSignal<StartOptionRow[]>([]);
const [gapRows, setGapRows] = createSignal<StartGapRow[]>([]);
const [blockingRows, setBlockingRows] = createSignal<StartBlockingRow[]>([]);
const [layoutOptions, setLayoutOptions] = createSignal<StartOptionRow[]>([]);
const [layoutRows, setLayoutRows] = createSignal<StartLayoutRow[]>([]);
const [slotOptions, setSlotOptions] = createSignal<StartOptionRow[]>([]);

// ── Applying a payload ──────────────────────────────────────────────────────

/** Write one /api/start payload into every signal (flash excluded — flash is
 *  one-shot action feedback, owned by `applyFlash`). Safe to call before
 *  adoption AND per poll. Copies; derives nothing. */
export function applyStart(p: StartPayload): void {
  const l = p.lines;
  const f = p.flags;
  const r = p.rows;

  setSessionLine(p.session.line);
  setDeviceLine(l.device_line);
  setDeviceDetail(l.device_detail);
  setBoardsLine(l.boards_line);
  setControllerLine(l.controller_line);
  setXinputLine(l.xinput_line);
  setBlockingLine(l.blocking_line);
  setPresetLine(l.preset_line);
  setMapperLine(l.mapper_line);
  setReadyLine(l.ready_line);
  setPlayLine(l.play_line);
  setGuideLine(l.guide_line);
  setStageError(l.stage_error);
  setScanError(l.scan_error);
  setPresetsError(l.presets_error);
  // §3's two must-says, straight off the staged view. Not composed here and
  // not composed in the seam either — they are `ksx_api::ESCAPE_HATCH_LINE`
  // and `BLOCKING_SCOPE_LINE`.
  setEscapeLine(p.staged.escape_hatch);
  setScopeLine(p.staged.blocking_scope);
  setNextPreset(p.staged.next_preset ?? "");

  setPillRunning(f.pill_running);
  setPillIdle(f.pill_idle);
  setPillDown(f.pill_down);
  setStageDown(f.stage_down);
  setScanDown(f.scan_down);
  setPresetsDown(f.presets_down);
  setHasDevice(f.has_device);
  setHasBoards(f.has_boards);
  setNoBoards(f.no_boards);
  setHasOther(f.has_other);
  setHasNotes(f.has_notes);
  setHasSlots(f.has_slots);
  setCanAdd(f.can_add);
  setSlotsFull(f.slots_full);
  setHasGaps(f.has_gaps);
  setCanLayout(f.can_layout);
  setBlockingAnswered(f.blocking_answered);
  setReady(f.ready);
  setNotReady(f.not_ready);
  setCanDiscard(f.can_discard);
  setSessionLive(f.session_live);

  setBoardRows(r.boards);
  setOtherRows(r.other);
  setNoteRows(r.notes);
  setSlotRows(r.slots);
  setPersonaOptions(r.personas);
  setGapRows(r.gaps);
  setBlockingRows(r.blocking);
  setLayoutOptions(r.layouts);
  setLayoutRows(r.layout_details);
  setSlotOptions(r.slot_numbers);
}

/** The studio server itself stopped answering /api/start. Say so and disable
 *  every verb — and do NOT clear the board list, because its rows are the last
 *  real reading of the machine and hiding them would look like an empty PC.
 *
 *  The wording here is the one thing this file owns, and it has no backend twin
 *  by definition: the backend is the thing not answering. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setStageDown(true);
  setStageError(
    "ksx Studio is not answering. Nothing below can be staged, saved or started until it does — and nothing on this page has been written, so there is nothing to undo.",
  );
  setCanAdd(false);
  setReady(false);
  setNotReady(true);
  setReadyLine(
    "ksx Studio is not answering, so neither Save nor Play can be performed.",
  );
  setCanDiscard(false);
}

/** One-shot action feedback (POST outcome or the seed's ?flash= value). */
const FLASH_MS = 8000;
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

export function StartIsland() {
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
        h("a", { class: "navlink on", href: "/start", "aria-current": "page" }, "Start"),
        h("a", { class: "navlink", href: "/" }, "Status"),
        h("a", { class: "navlink", href: "/map" }, "Mapper"),
        h("a", { class: "navlink", href: "/check" }, "Check"),
        h("a", { class: "navlink", href: "/pads" }, "Pads"),
        h("a", { class: "navlink", href: "/devices" }, "Devices"),
        h("a", { class: "navlink", href: "/profiles" }, "Profiles"),
        h("a", { class: "navlink", href: "/setup" }, "Setup"),
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
      // ── The three failed reads. Each says what did not happen; none of
      // them draws an empty machine. ──────────────────────────────────────
      createShow(
        () => stageDown(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            h("h2", null, "No staged setup"),
            h("p", { class: "alarmlead" }, () => stageError()),
            h(
              "p",
              { class: "alarmlead" },
              "The setup you build here lives in the ksx daemon for the length of ",
              "your visit — not in this page and not in a file. Start it from the ",
              "tray icon, and this screen picks up where it left off.",
            ),
          ),
      ),
      createShow(
        () => scanDown(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            h("h2", null, "Your devices could not be read"),
            h("p", { class: "alarmlead" }, () => scanError()),
            h(
              "p",
              { class: "alarmlead" },
              "This is not a reading of an empty machine — nothing was read at all, ",
              "so no list below is evidence about what is plugged in.",
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
      // ── STEP 1 (moment 4): CHOOSE A KEYBOARD ────────────────────────────
      h(
        "section",
        { class: "card wide dv-card" },
        h("h2", null, "1 · Choose a keyboard"),
        h("p", { class: "cardline" }, () => deviceLine()),
        createShow(
          () => hasDevice(),
          () =>
            h(
              "p",
              { class: "dv-line mono" },
              () => deviceDetail(),
            ),
        ),
        h("p", { class: "dv-note" }, () => boardsLine()),
        createShow(
          () => hasBoards(),
          () =>
            h(
              "p",
              { class: "dv-line" },
              "Each one says what ksx can do with it, because that is not ",
              "guessable from the name: a Bluetooth keyboard can be split but ",
              "never taken off the Windows keyboard stack, and a board with no ",
              "keyboard interface is not on this list at all.",
            ),
        ),
        h(
          "ul",
          { class: "plist dv-list" },
          createList(
            () => boardRows(),
            // KEY EVERY FIELD THE ROW RENDERS. forma reconciles by key and does
            // not patch a row whose key matched, so any member missing from the
            // key freezes at its first paint — render_start.rs's
            // `every_list_row_reconciles_on_every_field_it_renders` reads this
            // file and fails on one that is.
            (b) =>
              b.name +
              "|" +
              b.transport +
              "|" +
              b.backends +
              "|" +
              b.verdict +
              "|" +
              b.caveat +
              "|" +
              b.caveat_cls +
              "|" +
              b.cannot_type +
              "|" +
              b.cannot_type_cls +
              "|" +
              b.path +
              "|" +
              b.selector +
              "|" +
              b.alias +
              "|" +
              b.chosen_cls +
              "|" +
              b.button,
            (b) =>
              h(
                "li",
                { class: "dv-row" },
                h(
                  "div",
                  { class: "dv-head" },
                  // THE NAME, and nothing else, is the identifier on screen.
                  h("span", { class: "dv-name" }, b.name),
                  h("span", { class: "pill pill-idle" }, b.transport),
                  h("span", { class: b.chosen_cls }, "chosen"),
                ),
                h("p", { class: "dv-note" }, b.verdict),
                // What it can DO, because it is not guessable: a Bluetooth
                // keyboard can be split but never WinUSB-claimed.
                h("p", { class: "dv-line" }, b.backends),
                h("p", { class: b.caveat_cls }, b.caveat),
                h("p", { class: b.cannot_type_cls }, b.cannot_type),
                // SMALL PRINT, for a support conversation. Never the name of
                // the thing on this screen (FIRST-RUN.md §5), and never
                // something anyone is asked to type (§6).
                h(
                  "details",
                  { class: "st-more" },
                  h("summary", null, "Windows device path (for support)"),
                  h("p", { class: "dv-line mono" }, b.path),
                ),
                h(
                  "form",
                  { class: "dv-form", method: "post", action: "/start/device" },
                  h("input", { type: "hidden", name: "selector", value: b.selector }),
                  h("input", { type: "hidden", name: "alias", value: b.alias }),
                  h("input", { type: "hidden", name: "label", value: b.name }),
                  h("button", { class: "btn btn-primary", type: "submit" }, b.button),
                ),
              ),
          ),
        ),
        // "There is nothing here" — licensed by ONE flag, the one that is only
        // ever true when the enumeration actually answered.
        createShow(
          () => noBoards(),
          () =>
            h(
              "p",
              { class: "dv-note" },
              "No board on this PC exposes a keyboard interface, so there is ",
              "nothing ksx can split. Plug in the keyboard or arcade encoder you ",
              "want to use and press Rescan.",
            ),
        ),
        h(
          "p",
          { class: "pactrow" },
          // A GET, and it writes nothing: it re-reads the machine. The list is
          // never cached, so arriving here IS a scan — this is the visible
          // control FIRST-RUN.md §5 asks for, so nobody has to know one exists.
          h("a", { class: "btn btn-ghost", href: "/start" }, "Rescan"),
        ),
        createShow(
          () => hasOther(),
          () =>
            h(
              "details",
              { class: "st-more" },
              h("summary", null, "Devices that cannot be picked, and why"),
              h(
                "ul",
                { class: "plist dv-list" },
                createList(
                  () => otherRows(),
                  (o) => o.name + "|" + o.transport + "|" + o.reason + "|" + o.backends,
                  (o) =>
                    h(
                      "li",
                      { class: "dv-row quiet" },
                      h("span", { class: "dv-name" }, o.name),
                      h("span", { class: "pill pill-idle" }, o.transport),
                      h("span", { class: "dv-line" }, o.reason),
                      h("span", { class: "dv-line" }, o.backends),
                    ),
                ),
              ),
            ),
        ),
        createShow(
          () => hasNotes(),
          () =>
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => noteRows(),
                (n) => n.text,
                (n) => h("li", { class: "dv-row quiet" }, h("span", { class: "dv-line" }, n.text)),
              ),
            ),
        ),
      ),
      // ── STEP 2 (moment 5): CHOOSE A CONTROLLER ──────────────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "2 · Choose a controller"),
        h("p", { class: "cardline" }, () => controllerLine()),
        h(
          "ul",
          { class: "plist" },
          createList(
            () => slotRows(),
            (s) =>
              s.number +
              "|" +
              s.title +
              "|" +
              s.state +
              "|" +
              s.persona +
              "|" +
              s.xinput +
              "|" +
              s.preset +
              "|" +
              s.bindings,
            (s) =>
              h(
                "li",
                null,
                h(
                  "div",
                  { class: "pmeta" },
                  h("span", { class: "ptitle" }, s.title),
                  h("span", { class: "pill pill-ok" }, s.state),
                  h("span", { class: "pdetail" }, s.persona),
                  h("span", { class: "pdetail" }, s.xinput),
                  h("span", { class: "pdetail mono" }, s.preset),
                  h("span", { class: "pdetail" }, s.bindings),
                ),
                h(
                  "form",
                  { method: "post", action: "/start/controller/remove" },
                  h("input", { type: "hidden", name: "number", value: s.number }),
                  h("button", { class: "btn btn-ghost", type: "submit" }, "Remove"),
                ),
              ),
          ),
        ),
        createShow(
          () => hasSlots(),
          () => h("p", { class: "dv-line" }, () => xinputLine()),
        ),
        createShow(
          () => canAdd(),
          () =>
            h(
              "form",
              { class: "pactrow", method: "post", action: "/start/controller" },
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
              // The LAYOUT it starts from. Served, default first — so a user
              // who never opens this menu still gets a pad that does
              // something, which is the difference between "ready" and a
              // controller Play refuses.
              h(
                "label",
                { class: "bindlabel", for: "layout" },
                "starting from",
                h(
                  "select",
                  { id: "layout", name: "layout" },
                  createList(
                    () => layoutOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              // The preset name is SERVED, because it becomes a file name.
              // Nothing on this page asks anybody to type one.
              h("input", { type: "hidden", name: "preset", value: () => nextPreset() }),
              h("button", { class: "btn btn-primary", type: "submit" }, "Add this controller"),
            ),
        ),
        createShow(
          () => slotsFull(),
          () =>
            h(
              "p",
              { class: "warn" },
              "Every slot ksx has is staged. Remove one to stage a different ",
              "controller.",
            ),
        ),
        createShow(
          () => hasGaps(),
          () =>
            h(
              "details",
              { class: "st-more" },
              h("summary", null, "Controllers this build cannot create"),
              h(
                "ul",
                { class: "plist dv-list" },
                createList(
                  () => gapRows(),
                  (g) => g.label + "|" + g.gap + "|" + g.instead,
                  (g) =>
                    h(
                      "li",
                      { class: "dv-row" },
                      h("span", { class: "dv-name" }, g.label),
                      h("p", { class: "dv-note" }, g.gap),
                      h("p", { class: "dv-line" }, g.instead),
                    ),
                ),
              ),
            ),
        ),
      ),
      // ── STEP 3 (moment 6): MAP IT, AND THE ONE QUESTION ─────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "3 · Map it"),
        h("p", { class: "cardline" }, () => presetLine()),
        createShow(
          () => presetsDown(),
          () => h("p", { class: "warn" }, () => presetsError()),
        ),
        // GIVE A CONTROLLER A LAYOUT. Two selects and one submit, rather than
        // a form per staged row: a createList inside a createList is not a
        // shape this compiler emits, and the layout menu is the same menu for
        // every row.
        createShow(
          () => canLayout(),
          () =>
            h(
              "form",
              { class: "pactrow", method: "post", action: "/start/controller/layout" },
              h(
                "label",
                { class: "bindlabel", for: "layout-slot" },
                "give",
                h(
                  "select",
                  { id: "layout-slot", name: "number" },
                  createList(
                    () => slotOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h(
                "label",
                { class: "bindlabel", for: "layout-id" },
                "the layout",
                h(
                  "select",
                  { id: "layout-id", name: "layout" },
                  createList(
                    () => layoutOptions(),
                    (o) => o.value + "|" + o.label,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h("button", { class: "btn", type: "submit" }, "Use this layout"),
            ),
        ),
        h(
          "details",
          { class: "st-more" },
          h("summary", null, "What each layout expects"),
          h(
            "ul",
            { class: "plist dv-list" },
            createList(
              () => layoutRows(),
              (l) => l.label + "|" + l.panel + "|" + l.players,
              (l) =>
                h(
                  "li",
                  { class: "dv-row" },
                  h("span", { class: "dv-name" }, l.label),
                  h("p", { class: "dv-note" }, l.panel),
                  h("p", { class: "dv-line" }, l.players),
                ),
            ),
          ),
        ),
        h("p", { class: "dv-note" }, () => mapperLine()),
        h(
          "p",
          { class: "pactrow" },
          h("a", { class: "btn btn-ghost", href: "/map" }, "Open the mapper (edits saved files)"),
        ),
      ),
      h(
        "section",
        { class: "card wide warnbox" },
        h("h2", null, "Freeze this keyboard, or split it?"),
        h(
          "p",
          { class: "cardline" },
          "One question, and it decides whether you can still type while you ",
          "play. It is asked once and it is not asked again.",
        ),
        h("p", { class: "cardline" }, () => blockingLine()),
        h(
          "ul",
          { class: "plist dv-list" },
          createList(
            () => blockingRows(),
            (o) => o.name + "|" + o.title + "|" + o.detail + "|" + o.chosen_cls + "|" + o.button,
            (o) =>
              h(
                "li",
                { class: "dv-row" },
                h(
                  "div",
                  { class: "dv-head" },
                  h("span", { class: "dv-name" }, o.title),
                  h("span", { class: o.chosen_cls }, "answered"),
                ),
                h("p", { class: "dv-note" }, o.detail),
                h(
                  "form",
                  { class: "dv-form", method: "post", action: "/start/blocking" },
                  h("input", { type: "hidden", name: "blocking", value: o.name }),
                  h("button", { class: "btn", type: "submit" }, o.button),
                ),
              ),
          ),
        ),
        createShow(
          () => blockingAnswered(),
          () =>
            h(
              "p",
              { class: "dv-line" },
              "You can change this answer as often as you like — it is part of ",
              "the staged setup, so nothing is written until you save.",
            ),
        ),
        // The two things §3 requires on this screen, not buried. Both are
        // ksx-api's own sentences, arriving on the payload.
        h("p", { class: "dv-warn" }, () => escapeLine()),
        h("p", { class: "dv-note" }, () => scopeLine()),
      ),
      // ── STEP 4 (moment 7): PLAY ─────────────────────────────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "4 · Play"),
        h("p", { class: "cardline" }, () => readyLine()),
        h("p", { class: "cardline" }, () => playLine()),
        h(
          "p",
          { class: "cardline" },
          "Saving and playing are separate. Save writes config.toml and one ",
          "preset per controller; Play starts a session from what is on this ",
          "screen and writes nothing at all. Either works without the other.",
        ),
        createShow(
          () => sessionLive(),
          () =>
            h(
              "p",
              { class: "warn" },
              "A session is already running. Playing this setup replaces it — the ",
              "pads it plugged go, and the keyboards it captured are given back.",
            ),
        ),
        createShow(
          () => ready(),
          () =>
            h(
              "div",
              { class: "pactrow" },
              h(
                "form",
                { method: "post", action: "/start/save" },
                h("button", { class: "btn", type: "submit" }, "Save this setup"),
              ),
              h(
                "form",
                { method: "post", action: "/start/play" },
                h("button", { class: "btn btn-primary", type: "submit" }, "Play now"),
              ),
            ),
        ),
        createShow(
          () => notReady(),
          () =>
            h(
              "div",
              { class: "controls off" },
              h("button", { class: "btn", disabled: "" }, "Save this setup"),
              h("button", { class: "btn", disabled: "" }, "Play now"),
            ),
        ),
        h("p", { class: "dv-note" }, () => guideLine()),
        createShow(
          () => canDiscard(),
          () =>
            h(
              "p",
              { class: "pactrow" },
              h(
                "form",
                { method: "post", action: "/start/discard" },
                h("button", { class: "btn btn-ghost", type: "submit" }, "Start over"),
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
        "Nothing on this page claims a board, installs a driver or writes a ",
        "file until you press Save — and Play writes nothing even then. Session: ",
        h("span", { class: "mono" }, () => sessionLine()),
        ". Serving 127.0.0.1 only.",
      ),
    ),
  );
}
