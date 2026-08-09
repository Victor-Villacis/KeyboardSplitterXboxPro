import { h, createSignal, createList, createShow } from "@getforma/core";

// The SETUP island: the whole /setup screen, live.
//
// Same two-halves shape as StatusIsland (read its header for the islands
// protocol and the ledger rules); what follows is what is particular to this
// screen.
//
// # The config comes first, and it has exactly two verbs
//
// "the config should be first" and "why do we show the config root, we talked
// about this being seamless — only import and export etc."
//
// So: this page never puts a filesystem path in front of anyone as a thing to
// operate. IMPORT and EXPORT are the two verbs; a config root appears once, in
// small print, at the bottom of the inventory card, where a bug report can
// quote it. Everything else is a step or a fact.
//
// # The first run is a checklist the BACKEND decides
//
// `stepRows` is rendered, never computed here: `ksx_api::MachineSource::
// setup_state` returns the steps with their state already chosen (ksx-backend's
// `onboard::plan_steps`, pure and unit-tested), because "which step is next" is
// a decision about configuration and docs/SURFACES.md §1 puts decisions in the
// backend. The three ACTIONS below the list are authored, not derived — a list
// item body may only read members, so a row cannot branch between a link, a
// form and a button.
//
// Each action is one backend verb:
//   board → a link to /devices (the pick + name screen; one place, so a name
//           means the same thing everywhere)
//   slot  → POST /setup/slot     → ControlSource::assign_slot
//   prove → POST /setup/prove    → ControlSource::learn_start, and the page's
//           own poll reads learn_poll back. No JavaScript needed: the
//           <noscript> refresh re-renders the learner's state every 5 s.
//
// Compiler constraints honored below are StatusIsland's, unchanged.

// ── Wire types: serde field names from ksx-api's `machine` module and
//    crates/ksx-studio/src/{snapshot,control}.rs ─────────────────────────────

export interface SetupStep {
  id: string;
  title: string;
  detail: string;
  /** `done` | `now` | `later`. */
  state: string;
}

export interface SetupDeviceRow {
  alias: string;
  id: string;
  backend: string;
}

export interface SetupSlotRow {
  number: number;
  device: string;
  preset: string;
  persona: string;
  source: string;
}

export interface SetupView {
  generated_at: string;
  config_root: string;
  config_exists: boolean;
  devices: SetupDeviceRow[];
  slots: SetupSlotRow[];
  presets: string[];
  profiles: string[];
  steps: SetupStep[];
  notes: string[];
  /** The highest slot number this build accepts (`ksx_core::MAX_SLOTS`). The
   *  slot menu is built from it — this page does not get to pick a ceiling. */
  max_slots: number;
}

/** Every sentence the page states, composed in `crates/ksx-studio/src/
 *  snapshot.rs`. Rendered here, never derived here: two implementations of one
 *  sentence in two languages drift silently, and a test in one of them cannot
 *  see the other. */
export interface SetupLines {
  config: string;
  boards: string;
  slots: string;
  library: string;
  export: string;
  prove: string;
  /** Why the wire control is disabled — empty when it is not. */
  wire_blocked: string;
  /** Why the learner control is disabled — empty when it is not. */
  prove_blocked: string;
  /** What wiring a slot does to the pads of the session there actually is. */
  wire_warning: string;
}

/** Every `createShow` boolean on the page, decided in the same place and for
 *  the same reason. Assigned straight into the signals below. */
export interface SetupFlags {
  pill_running: boolean;
  pill_idle: boolean;
  pill_down: boolean;
  no_daemon: boolean;
  setup_down: boolean;
  setup_known: boolean;
  first_run: boolean;
  configured: boolean;
  can_wire: boolean;
  cannot_wire: boolean;
  prove_down: boolean;
  prove_listening: boolean;
  prove_hit: boolean;
  prove_idle: boolean;
  has_boards: boolean;
  no_boards: boolean;
  has_slots: boolean;
  no_slots: boolean;
  has_notes: boolean;
}

export interface SetupSnapshot {
  available: boolean;
  source: string;
  view: SetupView;
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

export interface LearnView {
  ok: boolean;
  /** `idle` | `listening` | `hit` | `unavailable`. */
  state: string;
  remaining_ms: number | null;
  device: string | null;
  key: string | null;
  error: string | null;
}

/** What GET /api/setup serves and what the island props carry — one shape
 *  (`SetupPayload` in snapshot.rs; parity unit-tested in render_setup.rs). */
export interface SetupPayload {
  setup: SetupSnapshot;
  session: SessionView;
  learn: LearnView;
  flash: string | null;
  /** Derived, server-side. See [[SetupLines]]. */
  lines: SetupLines;
  /** Derived, server-side. See [[SetupFlags]]. */
  flags: SetupFlags;
  /** Derived, server-side. See [[SetupRows]]. */
  rows: SetupRows;
}

interface StepRow {
  badge: string;
  title: string;
  detail: string;
  /** `step done` | `step now` | `step later` — server-picked, mirrored below. */
  cls: string;
}

interface RowPair {
  title: string;
  detail: string;
}

interface OptionRow {
  value: string;
  label: string;
}

interface TextRow {
  text: string;
}

/** Every list row this page draws, composed once in `snapshot.rs`
 *  (`SetupRows`) and assigned VERBATIM below. The row sentences and the
 *  slot-menu ceiling used to be formatted here and again in
 *  `render_setup.rs::list_values` — two languages, one sentence, silent
 *  drift. This page formats nothing. */
export interface SetupRows {
  steps: StepRow[];
  devices: RowPair[];
  slots: RowPair[];
  slot_options: OptionRow[];
  preset_options: TextRow[];
  profile_options: TextRow[];
  notes: TextRow[];
}

// ── The live state store (module-level: one island, page lifetime) ─────────

const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");
const [sessionLine, setSessionLine] = createSignal("not collected");
const [flashLine, setFlashLine] = createSignal("");
const [daemonCmd, setDaemonCmd] = createSignal("ksx daemon");
const [configLine, setConfigLine] = createSignal("not collected");
const [configRoot, setConfigRoot] = createSignal("(unknown)");
const [boardsSummary, setBoardsSummary] = createSignal("not collected");
const [slotsSummary, setSlotsSummary] = createSignal("not collected");
const [libraryLine, setLibraryLine] = createSignal("not collected");
const [exportLine, setExportLine] = createSignal("not collected");
const [proveLine, setProveLine] = createSignal("not collected");
const [proveKey, setProveKey] = createSignal("");
const [setupSource, setSetupSource] = createSignal("not collected");
const [wireBlocked, setWireBlocked] = createSignal("not collected");
const [proveBlocked, setProveBlocked] = createSignal("not collected");
const [wireWarning, setWireWarning] = createSignal("not collected");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [noDaemon, setNoDaemon] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [setupDown, setSetupDown] = createSignal(false);
// The checklist's own gate. Two names for one fact, because a signal used in
// two places on a page compiles to two slots with one name and the seam can
// only fill the first — the second would render its compile-time default for
// ever. One `createShow`, one signal, always.
const [stepsKnown, setStepsKnown] = createSignal(false);
const [stepsUnknown, setStepsUnknown] = createSignal(false);
const [firstRun, setFirstRun] = createSignal(false);
const [configured, setConfigured] = createSignal(false);
const [canWire, setCanWire] = createSignal(false);
const [cannotWire, setCannotWire] = createSignal(false);
const [proveIdle, setProveIdle] = createSignal(false);
const [proveListening, setProveListening] = createSignal(false);
const [proveHit, setProveHit] = createSignal(false);
const [proveDown, setProveDown] = createSignal(false);
const [hasBoards, setHasBoards] = createSignal(false);
const [noBoards, setNoBoards] = createSignal(false);
const [hasSlots, setHasSlots] = createSignal(false);
const [noSlots, setNoSlots] = createSignal(false);
const [hasNotes, setHasNotes] = createSignal(false);

const [stepRows, setStepRows] = createSignal<StepRow[]>([]);
const [deviceRows, setDeviceRows] = createSignal<RowPair[]>([]);
const [slotRows, setSlotRows] = createSignal<RowPair[]>([]);
const [slotOptions, setSlotOptions] = createSignal<OptionRow[]>([]);
const [presetOptions, setPresetOptions] = createSignal<TextRow[]>([]);
const [profileOptions, setProfileOptions] = createSignal<TextRow[]>([]);
const [noteRows, setNoteRows] = createSignal<TextRow[]>([]);

// ── Applying the payload. THERE ARE NO DERIVATIONS HERE ────────────────────
//
// Six display strings and the whole show-flag algebra used to be written twice
// — once in render_setup.rs for the SSR paint and again here for the poll —
// and the test that claimed to pin the two sides together only ever read the
// Rust one. So every sentence and every boolean below arrives on the payload,
// composed by `crates/ksx-studio/src/snapshot.rs`, and this function assigns
// them. docs/SURFACES.md §1: the decision lives in the backend; the view
// renders it.

/** Write one /api/setup payload into every signal (flash excluded — flash is
 *  one-shot action feedback, owned by `applyFlash`). */
export function applySetup(p: SetupPayload): void {
  const view = p.setup.view;
  const lines = p.lines;
  const f = p.flags;

  setGeneratedAt(view.generated_at === "" ? "(no snapshot)" : view.generated_at);
  setSessionLine(p.session.line);
  setDaemonCmd(p.session.profile ? `ksx daemon --game "${p.session.profile}"` : "ksx daemon");
  setConfigRoot(view.config_root === "" ? "(unknown)" : view.config_root);
  setSetupSource(p.setup.source);
  setProveKey(p.learn.key ?? "");

  setConfigLine(lines.config);
  setBoardsSummary(lines.boards);
  setSlotsSummary(lines.slots);
  setLibraryLine(lines.library);
  setExportLine(lines.export);
  setProveLine(lines.prove);
  setWireBlocked(lines.wire_blocked);
  setProveBlocked(lines.prove_blocked);
  setWireWarning(lines.wire_warning);

  setPillRunning(f.pill_running);
  setPillIdle(f.pill_idle);
  setPillDown(f.pill_down);
  setNoDaemon(f.no_daemon);
  setSetupDown(f.setup_down);
  setStepsKnown(f.setup_known);
  setStepsUnknown(f.setup_down);
  setFirstRun(f.first_run);
  setConfigured(f.configured);
  setCanWire(f.can_wire);
  setCannotWire(f.cannot_wire);
  setProveDown(f.prove_down);
  setProveListening(f.prove_listening);
  setProveHit(f.prove_hit);
  setProveIdle(f.prove_idle);
  setHasBoards(f.has_boards);
  setNoBoards(f.no_boards);
  setHasSlots(f.has_slots);
  setNoSlots(f.no_slots);
  setHasNotes(f.has_notes);

  // VERBATIM from `snapshot.rs::SetupRows` — no sentence and no ceiling is
  // composed on this side of the wire. (The slot menu once held its own
  // `SLOT_CHOICES = 8` here while `ksx_core::MAX_SLOTS` was 16, and the row
  // labels were formatted here and again in render_setup.rs; both are now
  // one Rust implementation that the SSR paint and this poll both read.)
  setStepRows(p.rows.steps);
  setDeviceRows(p.rows.devices);
  setSlotRows(p.rows.slots);
  setSlotOptions(p.rows.slot_options);
  setPresetOptions(p.rows.preset_options);
  setProfileOptions(p.rows.profile_options);
  setNoteRows(p.rows.notes);
}

/** The studio server itself stopped answering /api/setup.
 *
 *  The only state whose words are authored here rather than composed in Rust,
 *  and necessarily so: the process that composes them is the one that stopped
 *  answering. It takes the CONTROLS dead and says why; it deliberately leaves
 *  the inventory and the checklist showing the last read, because those were
 *  read successfully and a dropped poll is not evidence that they changed. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setNoDaemon(true);
  setCanWire(false);
  setCannotWire(true);
  setWireBlocked(
    "disabled — ksx Studio itself stopped answering, so nothing can be written from " +
      "this page. It retries every 2 s.",
  );
  setProveIdle(false);
  setProveListening(false);
  setProveHit(false);
  setProveDown(true);
  setProveBlocked(
    "disabled — ksx Studio itself stopped answering. It retries every 2 s.",
  );
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

export function SetupIsland() {
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
      // The same banner, word for word, as / and /map (render.rs
      // NO_DAEMON_HEADLINE is the oracle that keeps the three in step).
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
              "Import and Export below still work — they are the config store, not ",
              "the daemon. Wiring a slot and listening for a press both need one. ",
              "Two ways to start it:",
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
      // ── THE CONFIG, FIRST ─────────────────────────────────────────────
      h(
        "section",
        { class: "card hero setupcard" },
        h("h2", null, "Your configuration"),
        h("p", { class: "state" }, () => configLine()),
        // The daemon's state, quietly, because two of the three steps below
        // need one — and because this is where `applyUnreachable` writes when
        // ksx-studio itself stops answering.
        h("p", { class: "cardline mono" }, () => sessionLine()),
        createShow(
          () => flashOk(),
          () => h("p", { class: "flash flash-ok" }, () => flashLine()),
        ),
        createShow(
          () => flashError(),
          () => h("p", { class: "flash flash-err" }, () => flashLine()),
        ),
        createShow(
          () => setupDown(),
          () =>
            h(
              "p",
              { class: "warn" },
              "The configuration could not be read: ",
              h("span", { class: "mono" }, () => setupSource()),
            ),
        ),
        createShow(
          () => firstRun(),
          () =>
            h(
              "p",
              { class: "cardline" },
              "Nothing is set up yet. Either bring one in below — a file ksx ",
              "exported on another machine works as-is — or follow the three ",
              "steps and this cabinet will have one in a few minutes.",
            ),
        ),
        createShow(
          () => configured(),
          () =>
            h(
              "p",
              { class: "cardline" },
              "Take a copy before you change anything: Export writes one file you ",
              "can bring back with Import, on this machine or any other.",
            ),
        ),
        h(
          "div",
          { class: "controls setupverbs" },
          h(
            "a",
            {
              class: "btn btn-primary",
              href: "/setup/export.json",
              download: "",
            },
            "Export — download this configuration",
          ),
          h("a", { class: "btn", href: "#import" }, "Import — bring one in"),
        ),
        h("p", { class: "cardline mono" }, () => exportLine()),
      ),
      // ── FIRST RUN: the checklist ──────────────────────────────────────
      h(
        "section",
        { class: "card wide setupsteps" },
        h("h2", null, "Set this cabinet up"),
        // THE CHECKLIST IS A CLAIM ABOUT THIS MACHINE, so it is gated on the
        // read having worked. A refused provider used to render this heading,
        // the sentence below it and zero rows — which reads as "you have
        // nothing set up" when the truth is "I could not look".
        createShow(
          () => stepsKnown(),
          () =>
            h(
              "div",
              { class: "stepchecklist" },
              h(
                "p",
                { class: "cardline" },
                "Three steps, in order. ksx works out which one is next by reading ",
                "your configuration — there is nothing to tick off by hand.",
              ),
              h(
                "ol",
                { class: "steplist" },
                createList(
                  () => stepRows(),
                  (s) => s.badge + "|" + s.cls + "|" + s.title,
                  (s) =>
                    h(
                      "li",
                      { class: s.cls },
                      h("span", { class: "stepbadge" }, s.badge),
                      h(
                        "div",
                        { class: "stepbody" },
                        h("span", { class: "steptitle" }, s.title),
                        h("span", { class: "stepdetail" }, s.detail),
                      ),
                    ),
                ),
              ),
            ),
        ),
        createShow(
          () => stepsUnknown(),
          () =>
            h(
              "p",
              { class: "warn" },
              "ksx could not read this machine's configuration, so it cannot say ",
              "which step is next. That is not the same as nothing being set up ",
              "here — the card above says what could not be read. The three ",
              "actions below still work on their own.",
            ),
        ),
        // ── Step 1: the board. One place, and it is not this one. ───────
        h(
          "div",
          { class: "stepact" },
          h("h3", null, "1 · Find your board and name it"),
          h(
            "p",
            { class: "cardline" },
            "Boards are picked and named on the Devices screen, so a name means ",
            "the same thing everywhere ksx uses it.",
          ),
          h("a", { class: "btn", href: "/devices" }, "Go to Devices"),
          // The devices screen is task #22 and ships in the same merge as this
          // page (render_setup.rs "MERGE DEPENDENCY"). Naming the shell verb
          // beside the link is the same shape step 3 uses for a missing
          // listener: a step that cannot be taken here is never a dead end.
          h(
            "p",
            { class: "smallprint" },
            "No Devices screen in this build? ",
            h("code", { class: "mono copyable" }, "ksx device pick"),
            " does the same job in a shell.",
          ),
        ),
        // ── Step 2: wire a slot. One backend verb: slot-assign. ─────────
        h(
          "div",
          { class: "stepact" },
          h("h3", null, "2 · Wire a slot"),
          h(
            "p",
            { class: "cardline" },
            "A slot is one player: which preset it uses, and where that lives — ",
            "your config, or one game profile.",
          ),
          createShow(
            () => canWire(),
            () =>
              h(
                "form",
                { class: "controls", method: "post", action: "/setup/slot" },
                h("label", { for: "setup-slot" }, "slot"),
                h(
                  "select",
                  { id: "setup-slot", name: "slot" },
                  createList(
                    () => slotOptions(),
                    (o) => o.value,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
                h("label", { for: "setup-preset" }, "preset"),
                h(
                  "select",
                  { id: "setup-preset", name: "preset" },
                  createList(
                    () => presetOptions(),
                    (o) => o.text,
                    (o) => h("option", null, o.text),
                  ),
                ),
                h("label", { for: "setup-profile" }, "where"),
                h(
                  "select",
                  { id: "setup-profile", name: "profile" },
                  h("option", { value: "" }, "(this cabinet's config)"),
                  createList(
                    () => profileOptions(),
                    (o) => o.text,
                    (o) => h("option", null, o.text),
                  ),
                ),
                h("button", { class: "btn btn-primary", type: "submit" }, "Wire it"),
              ),
          ),
          createShow(
            () => cannotWire(),
            () =>
              h(
                "div",
                { class: "controls off" },
                h("button", { class: "btn", disabled: "" }, "Wire it"),
                // ONE reason, and it is the one that fired. `can_wire` is
                // three facts ANDed, and the single sentence that used to sit
                // here covered all of them at once — so it told a user with a
                // running daemon to start the daemon, and a user with presets
                // on disk to go and make a preset.
                h("p", { class: "warn" }, () => wireBlocked()),
              ),
          ),
          // What the write does to the session there IS. The unconditional
          // "REPLUGS the pads" was false against an idle daemon, which this
          // form is offered on.
          h("p", { class: "warn" }, () => wireWarning()),
        ),
        // ── Step 3: prove it. learn-start / learn-poll, the daemon's own. ─
        h(
          "div",
          { class: "stepact" },
          h("h3", null, "3 · Press a button and watch it land"),
          h("p", { class: "cardline" }, () => proveLine()),
          createShow(
            () => proveHit(),
            () => h("p", { class: "provekey mono" }, () => proveKey()),
          ),
          createShow(
            () => proveIdle(),
            () =>
              h(
                "form",
                { class: "controls", method: "post", action: "/setup/prove" },
                h("button", { class: "btn btn-primary", type: "submit" }, "Listen for a press"),
              ),
          ),
          createShow(
            () => proveListening(),
            () =>
              h(
                "form",
                { class: "controls", method: "post", action: "/setup/prove/cancel" },
                h("button", { class: "btn", type: "submit" }, "Stop listening"),
              ),
          ),
          createShow(
            () => proveDown(),
            () =>
              h(
                "div",
                { class: "controls off" },
                h("button", { class: "btn", disabled: "" }, "Listen for a press"),
                // Same rule: "no daemon" and "a daemon whose listener is not
                // available" are different states, and the page knows which.
                h("p", { class: "warn" }, () => proveBlocked()),
              ),
          ),
        ),
      ),
      // ── IMPORT: the second of the two verbs ───────────────────────────
      h(
        "section",
        { class: "card wide importcard", id: "import" },
        h("h2", null, "Import"),
        h(
          "p",
          { class: "cardline" },
          "Paste a configuration ksx exported — from this machine, another one, ",
          "or an assistant that wrote you one. Leave the box below unticked and ",
          "nothing is written: you get a report of exactly what it would do.",
        ),
        h(
          "form",
          { class: "importform", method: "post", action: "/setup/import" },
          h(
            "label",
            { class: "importlabel", for: "setup-document" },
            "the configuration, as JSON",
          ),
          h("textarea", {
            id: "setup-document",
            name: "document",
            class: "importbox mono",
            rows: "10",
            spellcheck: "false",
            placeholder: '{ "ksx_interop": 1, ... }',
          }),
          // A document ksx exported says what it is (`"ksx_interop": 1`), and
          // then this stays on "(the document says)". A BARE document — the
          // one an assistant writes you, which this card invites — carries no
          // envelope, and `read_bundle` refuses it in those words. This is how
          // the page says it, instead of refusing an invitation it made.
          h(
            "label",
            { class: "importlabel", for: "setup-what" },
            "if it is not a ksx export, what is it?",
          ),
          h(
            "select",
            { id: "setup-what", name: "what" },
            h("option", { value: "" }, "(the document says)"),
            h("option", { value: "config" }, "settings, boards and slots"),
            h("option", { value: "games" }, "game profiles"),
            h("option", { value: "presets" }, "presets"),
          ),
          h(
            "div",
            { class: "controls" },
            h(
              "label",
              { class: "importopt" },
              h("input", { type: "checkbox", name: "apply", value: "yes" }),
              " write it",
            ),
            h(
              "label",
              { class: "importopt" },
              h("input", { type: "checkbox", name: "force", value: "yes" }),
              " write even if it does not validate",
            ),
            h("button", { class: "btn btn-primary", type: "submit" }, "Import"),
          ),
        ),
        h(
          "p",
          { class: "cardline" },
          "Every file it replaces is copied first, so there is always a way back. ",
          "Comments in a replaced file do not survive the rewrite — that is the ",
          "price of an atomic, validated write, and it is why the copy is taken.",
        ),
        // Said here rather than only in a source comment: with scripting off
        // the form really posts and the page really reloads, so the box is
        // empty again. Every other control on this page is fully operable
        // without JavaScript; this one costs a second paste, and a person
        // about to lose a long document should hear it before they do.
        h(
          "p",
          { class: "smallprint" },
          "With JavaScript switched off this form still works, but the page ",
          "reloads to show the answer — so a dry run and then a write means ",
          "pasting the document twice. ",
          h("code", { class: "mono copyable" }, "ksx config import <file>"),
          " does it in one step from a shell.",
        ),
      ),
      // ── WHAT IS CONFIGURED: the inventory, quiet, last ────────────────
      h(
        "section",
        { class: "card sysinfo setupinv" },
        h("h2", null, "What is configured"),
        h("p", { class: "cardline" }, () => libraryLine()),
        h("p", { class: "cardline mono" }, () => boardsSummary()),
        createShow(
          () => hasBoards(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => deviceRows(),
                (d) => d.title + "|" + d.detail,
                (d) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, d.title),
                      h("span", { class: "pdetail mono" }, d.detail),
                    ),
                  ),
              ),
            ),
        ),
        createShow(
          () => noBoards(),
          () =>
            h(
              "p",
              { class: "ddetail" },
              "No board has a name yet — step 1 above is how one gets one.",
            ),
        ),
        h("p", { class: "cardline mono" }, () => slotsSummary()),
        createShow(
          () => hasSlots(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => slotRows(),
                (s) => s.title + "|" + s.detail,
                (s) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, s.title),
                      h("span", { class: "pdetail" }, s.detail),
                    ),
                  ),
              ),
            ),
        ),
        createShow(
          () => noSlots(),
          () =>
            h(
              "p",
              { class: "ddetail" },
              "No slot is wired yet — step 2 above is how one gets wired.",
            ),
        ),
        createShow(
          () => hasNotes(),
          () =>
            h(
              "ul",
              { class: "plist notelist" },
              createList(
                () => noteRows(),
                (n) => n.text,
                (n) => h("li", null, h("span", { class: "pdetail" }, n.text)),
              ),
            ),
        ),
        // The ONE place a path appears on this screen, and it is not an
        // interface: it is what a bug report quotes.
        h(
          "p",
          { class: "smallprint" },
          "For support: this cabinet's files live in ",
          h("span", { class: "mono" }, () => configRoot()),
          ". Nothing on this page needs you to go there.",
        ),
      ),
    ),
    h(
      "footer",
      null,
      h(
        "p",
        null,
        "Setup re-read every 2 s in place; Import, Export and the checklist are ",
        "the config store, and need no daemon. Without JavaScript the page ",
        "auto-refreshes every 5 s instead. Generated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Serving 127.0.0.1 only.",
      ),
    ),
  );
}
