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
// setup_state` returns the steps with their state already chosen (ksx-app's
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

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [noDaemon, setNoDaemon] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [setupDown, setSetupDown] = createSignal(false);
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

// ── Derivations (mirror render_setup.rs; pinned there by unit tests) ───────

/** How many slot numbers the wire form offers. Mirrors render_setup.rs
 *  `SLOT_CHOICES`: enough for the eight-player cabinet ksx already drives,
 *  and not `MAX_SLOTS` — a dropdown of sixteen is a worse answer than a
 *  config file for the cabinet that needs sixteen. */
const SLOT_CHOICES = 8;

export function configSummary(snap: SetupSnapshot): string {
  // A provider that refused knows nothing about this machine, so it must not
  // claim there is no configuration — that sentence is advice, and it would be
  // the wrong advice. Mirrors render_setup.rs `config_summary`.
  if (!snap.available) return "The configuration could not be read.";
  const view = snap.view;
  if (!view.config_exists) {
    return "There is no configuration on this machine yet.";
  }
  return `Configured — ${view.devices.length} board(s), ${view.slots.length} slot(s), ${view.presets.length} preset(s).`;
}

export function boardsLine(count: number): string {
  if (count === 0) return "no boards named yet";
  if (count === 1) return "1 board named:";
  return `${count} boards named:`;
}

export function slotsLine(count: number): string {
  if (count === 0) return "no slots wired yet";
  if (count === 1) return "1 slot wired:";
  return `${count} slots wired:`;
}

export function libraryLineOf(view: SetupView): string {
  return `${view.presets.length} preset(s) and ${view.profiles.length} game profile(s) on disk.`;
}

export function exportLineOf(view: SetupView): string {
  return `One JSON file: settings, boards, slots, ${view.profiles.length} game profile(s) and ${view.presets.length} preset(s).`;
}

export function learnLine(learn: LearnView): string {
  if (learn.state === "listening") {
    return "Listening — press any button on the panel now.";
  }
  if (learn.state === "hit") {
    return learn.device
      ? `Seen, on ${learn.device}.`
      : "Seen.";
  }
  if (learn.state === "unavailable" || !learn.ok) {
    return learn.error ?? "the daemon's listener is not available";
  }
  return "Nothing is listening. Start the listener, then press a button on the panel.";
}

/** Write one /api/setup payload into every signal (flash excluded — flash is
 *  one-shot action feedback, owned by `applyFlash`). */
export function applySetup(p: SetupPayload): void {
  const snap = p.setup;
  const view = snap.view;
  const session = p.session;
  const learn = p.learn;

  setGeneratedAt(view.generated_at === "" ? "(no snapshot)" : view.generated_at);
  setSessionLine(session.line);
  setDaemonCmd(session.profile ? `ksx daemon --game "${session.profile}"` : "ksx daemon");
  setConfigLine(configSummary(snap));
  setConfigRoot(view.config_root === "" ? "(unknown)" : view.config_root);
  setBoardsSummary(boardsLine(view.devices.length));
  setSlotsSummary(slotsLine(view.slots.length));
  setLibraryLine(libraryLineOf(view));
  setExportLine(exportLineOf(view));
  setSetupSource(snap.source);

  setProveLine(learnLine(learn));
  setProveKey(learn.key ?? "");

  const startable = session.reachable && !session.running;
  setPillRunning(session.reachable && session.running);
  setPillIdle(startable);
  setPillDown(!session.reachable);
  setNoDaemon(!session.reachable);

  setSetupDown(!snap.available);
  setFirstRun(snap.available && !view.config_exists);
  setConfigured(snap.available && view.config_exists);

  const wireable = session.reachable && view.presets.length > 0;
  setCanWire(wireable);
  setCannotWire(!wireable);

  setProveDown(!session.reachable || learn.state === "unavailable");
  setProveListening(session.reachable && learn.state === "listening");
  setProveHit(session.reachable && learn.state === "hit");
  setProveIdle(
    session.reachable &&
      learn.state !== "unavailable" &&
      learn.state !== "listening" &&
      learn.state !== "hit",
  );

  setHasBoards(view.devices.length > 0);
  setNoBoards(view.devices.length === 0);
  setHasSlots(view.slots.length > 0);
  setNoSlots(view.slots.length === 0);
  setHasNotes(view.notes.length > 0);

  setStepRows(
    view.steps.map((step, i) => ({
      badge: String(i + 1),
      title: step.title,
      detail: step.detail,
      cls: `step ${step.state}`,
    })),
  );
  setDeviceRows(
    view.devices.map((device) => ({
      title: device.alias,
      detail: `${device.backend} · ${device.id}`,
    })),
  );
  setSlotRows(
    view.slots.map((slot) => ({
      title: `Slot ${slot.number} — ${slot.preset}`,
      detail: `${slot.device} · ${slot.persona} · ${slot.source}`,
    })),
  );
  const choices: OptionRow[] = [];
  for (let n = 1; n <= SLOT_CHOICES; n++) {
    choices.push({ value: String(n), label: `Slot ${n}` });
  }
  setSlotOptions(choices);
  setPresetOptions(view.presets.map((name) => ({ text: name })));
  setProfileOptions(view.profiles.map((title) => ({ text: title })));
  setNoteRows(view.notes.map((text) => ({ text })));
}

/** The studio server itself stopped answering /api/setup. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setNoDaemon(true);
  setCanWire(false);
  setCannotWire(true);
  setProveIdle(false);
  setProveListening(false);
  setProveHit(false);
  setProveDown(true);
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
        h("a", { class: "navlink", href: "/" }, "Status"),
        h("a", { class: "navlink", href: "/map" }, "Mapper"),
        h("a", { class: "navlink", href: "/devices" }, "Devices"),
        h("a", { class: "navlink", href: "/profiles" }, "Profiles"),
        h("a", { class: "navlink on", href: "/setup", "aria-current": "page" }, "Setup"),
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
                h(
                  "p",
                  { class: "warn" },
                  "disabled — wiring a slot is a daemon write, and it needs a ",
                  "preset to point at. Start the daemon, and import or create a ",
                  "preset first.",
                ),
              ),
          ),
          h(
            "p",
            { class: "warn" },
            "Wiring a slot REPLUGS the pads: every controller vanishes and comes ",
            "back, and anything mid-game sees it. Bindings do not — those swap in ",
            "place.",
          ),
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
                h(
                  "p",
                  { class: "warn" },
                  "disabled — the listener lives in the daemon. Without one, ",
                  "`ksx monitor` does the same job in a shell.",
                ),
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
