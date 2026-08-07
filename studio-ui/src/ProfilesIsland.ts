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

// ── Row view models — DERIVED BY THE SERVER (snapshot.rs ProfilesDerived) ──
//
// These used to be built here, from functions that were a second copy of
// render_profiles.rs's. That is what docs/SURFACES.md §1 forbids, and the
// second copy went stale in exactly the way the rule predicts: the slot
// ceiling below was the literal string "16", `setMaxSlots` was never called,
// and no payload field could correct it — so the first `ksx_core::MAX_SLOTS`
// raise would have had the server render max="32" and hydration write 16 back
// over it (adoption effects write signal state into the DOM immediately; see
// the ledger-#5 note in profiles.ts). Now the island composes nothing: every
// string and every branch below arrives in `payload.view`.

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

interface TemplateRowView {
  id: string;
  label: string;
  /** The panel note that travels with the template — served since the
   *  beginning, rendered nowhere until the review asked why. */
  detail: string;
  players: string;
}

interface OptionView {
  value: string;
  label: string;
}

interface NoteView {
  line: string;
}

/** Everything this page displays that is not verbatim provider data, computed
 *  once in Rust (`ProfilesDerived`). Field names are the serde names. */
export interface ProfilesDerived {
  profiles_summary: string;
  broken_summary: string;
  presets_summary: string;
  templates_summary: string;
  daemon_cmd: string;
  /** `ksx_core::MAX_SLOTS`. The ONE place this number comes from. */
  max_slots: number;
  max_player: number;
  profile_rows: ProfileRowView[];
  broken_rows: BrokenRowView[];
  preset_rows: PresetRowView[];
  template_rows: TemplateRowView[];
  preset_options: OptionView[];
  template_options: OptionView[];
  note_rows: NoteView[];
  pill_running: boolean;
  pill_idle: boolean;
  pill_down: boolean;
  no_daemon: boolean;
  any_broken: boolean;
  rows_live: boolean;
  rows_plain: boolean;
  /** The games.toml read REFUSED — not "there are no profiles". */
  profiles_unreadable: boolean;
  can_make_profile: boolean;
  no_presets_yet: boolean;
  /** The presets read REFUSED — not "there are no presets". */
  presets_unreadable: boolean;
  can_make_preset: boolean;
  any_notes: boolean;
}

/** What GET /api/profiles serves and what the island props carry — one shape
 *  (`ProfilesPayload` in snapshot.rs; the parity is unit-tested there). */
export interface ProfilesPayload {
  profiles: ProfilesView;
  presets: PresetsView;
  session: SessionView;
  /** The refusal that stopped the games.toml read, if it stopped. `null` and
   *  `"…could not be read"` are different sentences and the user acts on them
   *  differently — which is why this is a field and not an empty list. */
  profiles_error: string | null;
  presets_error: string | null;
  notes: string[];
  flash: string | null;
  view: ProfilesDerived;
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
const [profilesError, setProfilesError] = createSignal("");
const [presetsError, setPresetsError] = createSignal("");
// NO compile-time ceiling. These were `createSignal("16")` and `max: "4"`,
// and `setMaxSlots` was never called — a number that only LOOKED live. The
// server sends `ksx_core::MAX_SLOTS` and the widest template's player count in
// every payload; until one arrives, an empty `max` means "no client-side
// ceiling", and the backend refuses an out-of-range value in words. A wrong
// ceiling silently rejects a legal input; no ceiling does not.
const [maxSlots, setMaxSlots] = createSignal("");
const [maxPlayer, setMaxPlayer] = createSignal("");

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
const [profilesUnreadable, setProfilesUnreadable] = createSignal(false);
const [canMakeProfile, setCanMakeProfile] = createSignal(false);
const [noPresetsYet, setNoPresetsYet] = createSignal(false);
const [presetsUnreadable, setPresetsUnreadable] = createSignal(false);
const [canMakePreset, setCanMakePreset] = createSignal(false);

const [profileRows, setProfileRows] = createSignal<ProfileRowView[]>([]);
const [brokenRows, setBrokenRows] = createSignal<BrokenRowView[]>([]);
const [presetRows, setPresetRows] = createSignal<PresetRowView[]>([]);
const [templateRows, setTemplateRows] = createSignal<TemplateRowView[]>([]);
const [presetOptions, setPresetOptions] = createSignal<OptionView[]>([]);
const [templateOptions, setTemplateOptions] = createSignal<OptionView[]>([]);
const [noteRows, setNoteRows] = createSignal<NoteView[]>([]);

/** Write one /api/profiles payload into every signal (flash excluded — flash
 *  is one-shot action feedback, owned by `applyFlash`).
 *
 *  Copy, and nothing else. Every sentence, every pill class, every count and
 *  both numeric ceilings arrive in `p.view`, composed once by `snapshot.rs`.
 *  This function deriving ANY of them again would be the drift docs/SURFACES.md
 *  §1 bans — and the last time it did, the drift was a hardcoded slot ceiling
 *  the server could not reach. */
export function applyProfiles(p: ProfilesPayload): void {
  const d = p.view;

  setGeneratedAt(p.profiles.generated_at);
  setSessionLine(p.session.line);
  setDaemonCmd(d.daemon_cmd);
  setGamesPath(p.profiles.games_path);
  setPresetRoot(p.presets.config_root);
  setProfilesSummary(d.profiles_summary);
  setBrokenSummary(d.broken_summary);
  setPresetsSummary(d.presets_summary);
  setTemplatesSummary(d.templates_summary);
  setProfilesError(p.profiles_error ?? "");
  setPresetsError(p.presets_error ?? "");
  setMaxSlots(String(d.max_slots));
  setMaxPlayer(String(d.max_player));

  setPillRunning(d.pill_running);
  setPillIdle(d.pill_idle);
  setPillDown(d.pill_down);
  setNoDaemon(d.no_daemon);
  setRowsLive(d.rows_live);
  setRowsPlain(d.rows_plain);
  setAnyBroken(d.any_broken);
  setProfilesUnreadable(d.profiles_unreadable);
  setCanMakeProfile(d.can_make_profile);
  setNoPresetsYet(d.no_presets_yet);
  setPresetsUnreadable(d.presets_unreadable);
  setCanMakePreset(d.can_make_preset);
  setAnyNotes(d.any_notes);

  setBrokenRows(d.broken_rows);
  setProfileRows(d.profile_rows);
  setPresetRows(d.preset_rows);
  setTemplateRows(d.template_rows);
  setPresetOptions(d.preset_options);
  setTemplateOptions(d.template_options);
  setNoteRows(d.note_rows);
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
        // A REFUSED read is not an empty list. Before this box existed, an
        // unreadable games.toml printed "no profiles in games.toml" here and
        // put the reason in the last card on the page — a page telling you
        // your cabinet is empty when what actually happened is that it could
        // not look. The summary line above says so too; this says why.
        createShow(
          () => profilesUnreadable(),
          () =>
            h(
              "div",
              { class: "warnbox" },
              h("p", { class: "warn" }, () => profilesError()),
              h(
                "p",
                { class: "cardline" },
                "Nothing below this line is a statement about your profiles. ",
                "The file ksx tried to read is ",
                h("span", { class: "mono" }, () => gamesPath()),
                ".",
              ),
            ),
        ),
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
                  // ksx_core::MAX_SLOTS, injected. Never a literal.
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
        // The THIRD state, and the reason it is not the one above: "make a
        // preset below first" points at a form whose template <select> is fed
        // by the same read that just failed. Offering it would be a closed
        // loop — the only route out of the empty state cannot succeed — with
        // a sentence on it that claims to know the folder is empty.
        createShow(
          () => presetsUnreadable(),
          () =>
            h(
              "div",
              { class: "warnbox" },
              h("p", { class: "warn" }, () => presetsError()),
              h(
                "p",
                { class: "cardline" },
                "This is not an empty presets folder — it is a read that ",
                "refused, so ksx does not know what is in it. Both forms on ",
                "this page are withheld until it can be read: creating a ",
                "profile needs a preset that exists, and creating a preset ",
                "needs to know the name is free.",
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
          "mapper edits them. `default` and `empty` are built in — the ",
          "\"built-in\" pill marks them — but the pill is not what protects a ",
          "file: creating from this page never overwrites ANYTHING, yours ",
          "included. A name that is already taken is refused, and the refusal ",
          "names `--force`, which is the CLI's consent step for replacing a ",
          "preset (it takes a timestamped backup first).",
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
        // The summary above ends in a colon and used to be followed by a
        // FORM. `TemplateRow.detail` — the panel note ksx-api describes as
        // the thing without which "a template nobody can identify from a list
        // is a template nobody uses" — was transmitted on every request and
        // rendered nowhere. Here is the list the colon promised.
        h(
          "ul",
          { class: "plist" },
          createList(
            () => templateRows(),
            (t) => t.id,
            (t) =>
              h(
                "li",
                null,
                h(
                  "div",
                  { class: "pmeta" },
                  h("span", { class: "ptitle" }, t.id),
                  h("span", { class: "pdetail" }, t.label),
                  h("span", { class: "pdetail" }, t.detail),
                ),
                h("span", { class: "pill pill-idle" }, t.players),
              ),
          ),
        ),
        createShow(
          () => canMakePreset(),
          () =>
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
                "player block — each option above names the range it has",
                h("input", {
                  id: "npr-player",
                  type: "number",
                  name: "player",
                  min: "1",
                  // The widest block any offered template carries, injected.
                  // It was the literal "4", which matched whichever template
                  // happened to be widest rather than the one selected — so
                  // `keyboard-2p` + player 3 was offerable and refused
                  // server-side. One ceiling cannot express four templates;
                  // the per-template range is in the option label instead,
                  // and the backend still refuses what it must.
                  max: () => maxPlayer(),
                  value: "1",
                }),
              ),
              h(
                "button",
                { class: "btn btn-primary", type: "submit" },
                "Create preset",
              ),
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
