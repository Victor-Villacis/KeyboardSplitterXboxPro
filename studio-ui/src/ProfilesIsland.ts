import { h, createSignal, createList, createShow } from "@getforma/core";

// The island: the whole Profiles screen, live.
//
// Same two halves as StatusIsland (read its header for the protocol): the
// `createSignal` declarations below ARE the FMIR slot table, and the same
// signals are rewritten every 2 s from `GET /api/profiles`.
//
// What this screen is for, in one sentence: **the two things a person cannot
// otherwise do without hand-editing TOML** — start a games.toml profile, and
// start a preset from an in-box template — plus the read that makes the first
// one honest.
//
// That read is the point of the page. `ksx_games::preflight` has always known
// that a profile's .exe is missing; it just ran at LAUNCH time, so a cabinet
// whose emulator moved looked perfectly healthy right up to the press of the
// button. `MachineSource::profiles` runs the identical check on the read side,
// so a broken profile is a row with the wrong path printed on it — here, now,
// next to the profile it belongs to.
//
// Compiler constraints honored below (see render.rs, and StatusIsland's list):
// dynamic text/attrs are bare `() => signalName()` calls; list sources are bare
// `() => listSignal()`; list item bodies use only direct member reads (which is
// why every row carries its own precomputed `statecls` — a `createShow` cannot
// live inside an item body); createShow conditions are bare getters.

// ── Wire types: serde field names from crates/ksx-api/src/machine.rs ────────

export interface ProfileDetail {
  title: string;
  path: string;
  arguments: string;
  slots: number;
  presets: string[];
  /** `ok` | `broken` | `launcher`. */
  state: string;
  verdict: string;
  /** Present only when state === "broken" — the path that is wrong. */
  broken_path?: string | null;
}

export interface ProfilesView {
  generated_at: string;
  config_root: string;
  games_path: string;
  profiles: ProfileDetail[];
  notes: string[];
}

export interface PresetRow {
  name: string;
  bound: number;
  macros: number;
  protected: boolean;
  source: string;
}

export interface TemplateRow {
  id: string;
  label: string;
  detail: string;
  players: number[];
}

export interface PresetsView {
  config_root: string;
  presets: PresetRow[];
  templates: TemplateRow[];
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

/** What GET /api/profiles serves and what the island props carry — one shape
 *  (`ProfilesPayload` in snapshot.rs; the parity is unit-tested there). */
export interface ProfilesPayload {
  profiles: ProfilesView;
  presets: PresetsView;
  session: SessionView;
  notes: string[];
  flash: string | null;
}

// ── Row view models (server derives the same fields; render_profiles.rs) ────

interface ProfileRowView {
  title: string;
  path: string;
  detail: string;
  verdict: string;
  statecls: string;
  statelabel: string;
}

interface BrokenRowView {
  title: string;
  /** The path that does not resolve. This is the whole reason the card
   *  exists: "MAME 4P is broken" without the string is a second search. */
  path: string;
  verdict: string;
}

interface PresetRowView {
  name: string;
  detail: string;
  statecls: string;
  statelabel: string;
}

interface OptionView {
  value: string;
  label: string;
}

interface NoteView {
  line: string;
}

// ── The live state store (module-level: one island, page lifetime) ─────────

const [generatedAt, setGeneratedAt] = createSignal("(no snapshot)");
const [sessionLine, setSessionLine] = createSignal("not collected");
const [flashLine, setFlashLine] = createSignal("");
const [daemonCmd, setDaemonCmd] = createSignal("ksx daemon");
const [gamesPath, setGamesPath] = createSignal("(unknown)");
const [presetRoot, setPresetRoot] = createSignal("(unknown)");
const [profilesSummary, setProfilesSummary] = createSignal("not collected");
const [brokenSummary, setBrokenSummary] = createSignal("");
const [presetsSummary, setPresetsSummary] = createSignal("not collected");
const [templatesSummary, setTemplatesSummary] = createSignal("not collected");
const [maxSlots, setMaxSlots] = createSignal("16");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [noDaemon, setNoDaemon] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [anyBroken, setAnyBroken] = createSignal(false);
const [anyNotes, setAnyNotes] = createSignal(false);
const [rowsLive, setRowsLive] = createSignal(false);
const [rowsPlain, setRowsPlain] = createSignal(false);
const [canMakeProfile, setCanMakeProfile] = createSignal(false);
const [noPresetsYet, setNoPresetsYet] = createSignal(false);

const [profileRows, setProfileRows] = createSignal<ProfileRowView[]>([]);
const [brokenRows, setBrokenRows] = createSignal<BrokenRowView[]>([]);
const [presetRows, setPresetRows] = createSignal<PresetRowView[]>([]);
const [presetOptions, setPresetOptions] = createSignal<OptionView[]>([]);
const [templateOptions, setTemplateOptions] = createSignal<OptionView[]>([]);
const [noteRows, setNoteRows] = createSignal<NoteView[]>([]);

// ── Derivations (mirror render_profiles.rs; pinned there by unit tests) ────

function profilesSummaryLine(count: number): string {
  if (count === 0) return "no profiles in games.toml";
  if (count === 1) return "1 profile in games.toml:";
  return `${count} profiles in games.toml:`;
}

function brokenSummaryLine(count: number): string {
  if (count === 1) return "1 profile points at a program that is not there:";
  return `${count} profiles point at a program that is not there:`;
}

function presetsSummaryLine(count: number): string {
  if (count === 0) return "no presets on disk";
  if (count === 1) return "1 preset on disk:";
  return `${count} presets on disk:`;
}

function templatesSummaryLine(count: number): string {
  if (count === 1) return "1 in-box template:";
  return `${count} in-box templates:`;
}

/** A profile's state as the pill that carries it. `launcher` is deliberately
 *  the neutral pill, not the OK one: preflight cannot check a `steam://` URL,
 *  and a green badge would claim a check ksx did not make. */
function stateClass(state: string): string {
  if (state === "broken") return "pill pill-warn";
  if (state === "launcher") return "pill pill-idle";
  return "pill pill-ok";
}

function profileDetailLine(p: ProfileDetail): string {
  const slots = p.slots === 1 ? "1 slot" : `${p.slots} slots`;
  if (p.presets.length === 0) return `${slots} — no preset named`;
  return `${slots} on ${p.presets.join(", ")}`;
}

function presetDetailLine(p: PresetRow): string {
  const controls = p.bound === 1 ? "1 control" : `${p.bound} controls`;
  const macros = p.macros === 0 ? "" : `, ${p.macros} macro(s)`;
  return `${controls}${macros} — ${p.source}`;
}

/** Write one /api/profiles payload into every signal (flash excluded — flash
 *  is one-shot action feedback, owned by `applyFlash`). */
export function applyProfiles(p: ProfilesPayload): void {
  const view = p.profiles;
  const presets = p.presets;
  const session = p.session;

  setGeneratedAt(view.generated_at);
  setSessionLine(session.line);
  setDaemonCmd(
    session.profile ? `ksx daemon --game "${session.profile}"` : "ksx daemon",
  );
  setGamesPath(view.games_path);
  setPresetRoot(presets.config_root);
  setProfilesSummary(profilesSummaryLine(view.profiles.length));
  setPresetsSummary(presetsSummaryLine(presets.presets.length));
  setTemplatesSummary(templatesSummaryLine(presets.templates.length));

  const startable = session.reachable && !session.running;
  setPillRunning(session.reachable && session.running);
  setPillIdle(startable);
  setPillDown(!session.reachable);
  setNoDaemon(!session.reachable);
  setRowsLive(startable);
  setRowsPlain(!startable);
  setCanMakeProfile(presets.presets.length > 0);
  setNoPresetsYet(presets.presets.length === 0);

  const broken = view.profiles.filter((g) => g.state === "broken");
  setAnyBroken(broken.length > 0);
  setBrokenSummary(brokenSummaryLine(broken.length));
  setBrokenRows(
    broken.map((g) => ({
      title: g.title,
      path: g.broken_path ?? g.path,
      verdict: g.verdict,
    })),
  );

  setProfileRows(
    view.profiles.map((g) => ({
      title: g.title,
      path: g.path,
      detail: profileDetailLine(g),
      verdict: g.verdict,
      statecls: stateClass(g.state),
      statelabel: g.state,
    })),
  );

  setPresetRows(
    presets.presets.map((r) => ({
      name: r.name,
      detail: presetDetailLine(r),
      statecls: r.protected ? "pill pill-idle" : "pill pill-ok",
      statelabel: r.protected ? "built-in" : "yours",
    })),
  );
  setPresetOptions(
    presets.presets.map((r) => ({ value: r.name, label: r.name })),
  );
  setTemplateOptions(
    presets.templates.map((t) => ({
      value: t.id,
      label: `${t.id} — ${t.label}`,
    })),
  );

  const notes = p.notes ?? [];
  setAnyNotes(notes.length > 0);
  setNoteRows(notes.map((line) => ({ line })));
}

/** The studio server itself stopped answering: say so and stop offering the
 *  controls, but keep the last-known lists on screen — their timestamp stops
 *  advancing, which is the honest tell. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setNoDaemon(true);
  setRowsLive(false);
  setRowsPlain(true);
}

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

export function ProfilesIsland() {
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
        h(
          "a",
          { class: "navlink on", href: "/profiles", "aria-current": "page" },
          "Profiles",
        ),
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
      // ── The banner every page carries, word for word. ────────────────
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
              "Everything below is a real reading of this machine. Creating a ",
              "profile or a preset writes to disk and works without a daemon; ",
              "SWITCHING to a profile starts a session, and that needs one. ",
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
      // ── BROKEN PROFILES: the headline, above everything it is about ──
      // A profile pointing at a program that is gone used to fail at launch
      // and nowhere else — the cabinet did nothing when the button was
      // pressed, and the only way to find out why was to read games.toml
      // against the filesystem by hand. Same check, moved to where a person
      // is already looking, with the path printed back.
      createShow(
        () => anyBroken(),
        () =>
          h(
            "section",
            { class: "card alarm warn" },
            h("h2", null, "Broken profiles"),
            h("p", { class: "alarmlead" }, () => brokenSummary()),
            h(
              "ul",
              { class: "plist" },
              createList(
                () => brokenRows(),
                (b) => b.title + "|" + b.path,
                (b) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, b.title),
                      h("span", { class: "pdetail" }, b.path),
                      h("span", { class: "pdetail" }, b.verdict),
                    ),
                  ),
              ),
            ),
            h(
              "p",
              { class: "cardline" },
              "Fix `path` for these in games.toml — the file is ",
              h("span", { class: "mono" }, () => gamesPath()),
              " — or make a new profile below and stop using the old one.",
            ),
          ),
      ),
      // ── SESSION line + flash ──────────────────────────────────────────
      h(
        "section",
        { class: "card hero session" },
        h("h2", null, "Session"),
        h("p", { class: "state" }, () => sessionLine()),
        createShow(
          () => flashOk(),
          () => h("p", { class: "flash flash-ok" }, () => flashLine()),
        ),
        createShow(
          () => flashError(),
          () => h("p", { class: "flash flash-err" }, () => flashLine()),
        ),
      ),
      // ── PROFILES ──────────────────────────────────────────────────────
      h(
        "section",
        { class: "card wide profilecard" },
        h("h2", null, "Profiles"),
        h(
          "p",
          { class: "cardline" },
          "Each profile is a games.toml entry: the program to launch and the ",
          "slots it hands out. Switching to one starts a session under it — ",
          "the same verb the tray and `ksx daemon --game` use.",
        ),
        h("p", { class: "cardline mono" }, () => profilesSummary()),
        // Two lists, one signal, one show pair — the status page's shape.
        // A Switch button is only offered when a start could actually be
        // accepted; a dead button rendered as live is the one thing this
        // page must not do.
        createShow(
          () => rowsLive(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => profileRows(),
                (g) => g.title + "|" + g.statelabel + "|" + g.path,
                (g) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, g.title),
                      h("span", { class: "pdetail" }, g.detail),
                      h("span", { class: "pdetail" }, g.path),
                      h("span", { class: "pdetail" }, g.verdict),
                    ),
                    h("span", { class: g.statecls }, g.statelabel),
                    h(
                      "form",
                      { method: "post", action: "/profiles/switch" },
                      h("input", {
                        type: "hidden",
                        name: "profile",
                        value: g.title,
                      }),
                      // One word: the row already names the profile, and a
                      // phone row that also carries a state pill has no
                      // width to spend on a sentence.
                      h(
                        "button",
                        { class: "btn btn-row", type: "submit" },
                        "Switch",
                      ),
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
                (g) => g.title + "|" + g.statelabel + "|" + g.path,
                (g) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "ptitle" }, g.title),
                      h("span", { class: "pdetail" }, g.detail),
                      h("span", { class: "pdetail" }, g.path),
                      h("span", { class: "pdetail" }, g.verdict),
                    ),
                    h("span", { class: g.statecls }, g.statelabel),
                  ),
              ),
            ),
        ),
      ),
      // ── NEW PROFILE — the thing that could not be done at all ─────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "New profile"),
        h(
          "p",
          { class: "cardline" },
          "Writes a [[game]] entry into games.toml and seeds one slot per ",
          "player, all on the preset you choose. The keyboard stays unset — ",
          "every board drives the slot until `ksx setup` wires a specific ",
          "one. A timestamped backup of games.toml is taken first.",
        ),
        createShow(
          () => canMakeProfile(),
          () =>
            h(
              "form",
              { class: "grid", method: "post", action: "/profiles/new" },
              h(
                "label",
                { class: "bindlabel", for: "np-title" },
                "title — the name `ksx run --game` takes",
                h("input", {
                  id: "np-title",
                  type: "text",
                  name: "title",
                  required: "",
                  placeholder: "Street Fighter",
                }),
              ),
              h(
                "label",
                { class: "bindlabel", for: "np-path" },
                "path — the .exe, or a launcher URL",
                h("input", {
                  id: "np-path",
                  type: "text",
                  name: "path",
                  required: "",
                  placeholder: "C:\\games\\sf.exe",
                }),
              ),
              h(
                "label",
                { class: "bindlabel", for: "np-args" },
                "arguments (optional)",
                h("input", {
                  id: "np-args",
                  type: "text",
                  name: "arguments",
                  placeholder: "-fullscreen",
                }),
              ),
              h(
                "label",
                { class: "bindlabel", for: "np-slots" },
                "slots — one per player",
                h("input", {
                  id: "np-slots",
                  type: "number",
                  name: "slots",
                  min: "1",
                  max: () => maxSlots(),
                  value: "2",
                }),
              ),
              h(
                "label",
                { class: "bindlabel", for: "np-preset" },
                "preset every slot starts on",
                h(
                  "select",
                  { id: "np-preset", name: "preset" },
                  createList(
                    () => presetOptions(),
                    (o) => o.value,
                    (o) => h("option", { value: o.value }, o.label),
                  ),
                ),
              ),
              h(
                "button",
                { class: "btn btn-primary", type: "submit" },
                "Create profile",
              ),
            ),
        ),
        createShow(
          () => noPresetsYet(),
          () =>
            h(
              "div",
              { class: "warnbox" },
              h(
                "p",
                { class: "warn" },
                "No presets on disk, and a profile's slots have to start on ",
                "one. Make a preset from an in-box template below first — the ",
                "form comes back the moment there is one.",
              ),
            ),
        ),
      ),
      // ── PRESETS ───────────────────────────────────────────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "Presets"),
        h(
          "p",
          { class: "cardline" },
          "A preset is one pad's key map. Slots point at them by name; the ",
          "mapper edits them. `default` and `empty` are built in and are ",
          "never overwritten.",
        ),
        h("p", { class: "cardline mono" }, () => presetsSummary()),
        h(
          "ul",
          { class: "plist" },
          createList(
            () => presetRows(),
            (r) => r.name + "|" + r.detail,
            (r) =>
              h(
                "li",
                null,
                h(
                  "div",
                  { class: "pmeta" },
                  h("span", { class: "ptitle" }, r.name),
                  h("span", { class: "pdetail" }, r.detail),
                ),
                h("span", { class: r.statecls }, r.statelabel),
              ),
          ),
        ),
      ),
      // ── NEW PRESET FROM TEMPLATE ──────────────────────────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "New preset from a template"),
        h(
          "p",
          { class: "cardline" },
          "The layouts that ship in the binary — an I-PAC on its factory ",
          "chart, MAME's four-player chart, a desk keyboard, and two players ",
          "sharing one keyboard. Instantiating one writes an ordinary, ",
          "editable preset file; from then on it is yours.",
        ),
        h("p", { class: "cardline mono" }, () => templatesSummary()),
        h(
          "form",
          { class: "grid", method: "post", action: "/profiles/preset/new" },
          h(
            "label",
            { class: "bindlabel", for: "npr-name" },
            "name for the new preset",
            h("input", {
              id: "npr-name",
              type: "text",
              name: "name",
              required: "",
              placeholder: "P1 panel",
            }),
          ),
          h(
            "label",
            { class: "bindlabel", for: "npr-template" },
            "template",
            h(
              "select",
              { id: "npr-template", name: "template" },
              createList(
                () => templateOptions(),
                (o) => o.value,
                (o) => h("option", { value: o.value }, o.label),
              ),
            ),
          ),
          h(
            "label",
            { class: "bindlabel", for: "npr-player" },
            "player block (multi-player templates only)",
            h("input", {
              id: "npr-player",
              type: "number",
              name: "player",
              min: "1",
              max: "4",
              value: "1",
            }),
          ),
          h(
            "button",
            { class: "btn btn-primary", type: "submit" },
            "Create preset",
          ),
        ),
        h(
          "p",
          { class: "cardline" },
          "presets are written to ",
          h("span", { class: "mono" }, () => presetRoot()),
        ),
      ),
      // ── NOTES: anything the reads had to say out loud ─────────────────
      createShow(
        () => anyNotes(),
        () =>
          h(
            "section",
            { class: "card" },
            h("h2", null, "Notes from the config read"),
            h(
              "ul",
              { class: "plist" },
              createList(
                () => noteRows(),
                (n) => n.line,
                (n) =>
                  h(
                    "li",
                    null,
                    h(
                      "div",
                      { class: "pmeta" },
                      h("span", { class: "pdetail" }, n.line),
                    ),
                  ),
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
        "Profiles and presets re-read every 2 s in place; every button is one ",
        "backend verb. Without JavaScript the page auto-refreshes every 5 s ",
        "instead. Generated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Serving 127.0.0.1 only.",
      ),
    ),
  );
}
