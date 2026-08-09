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
  revision: string;
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
  usable: boolean;
  problem?: string | null;
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
  revision: string;
  title: string;
  path: string;
  arguments: string;
  slots: string;
  max_slots: string;
  preset: string;
  layout_options: OptionView[];
  detail: string;
  verdict: string;
  statecls: string;
  statelabel: string;
  play_disabled: boolean;
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
  /** The concise template-card intro. The full served roster lives in the
   *  optional comparison disclosure below. */
  templates_intro: string;
  play_status: string;
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
  can_stop: boolean;
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
const [templatesIntro, setTemplatesIntro] = createSignal("not collected");
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
const [canStop, setCanStop] = createSignal(false);

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
  setSessionLine(d.play_status);
  setDaemonCmd(d.daemon_cmd);
  setGamesPath(p.profiles.games_path);
  setPresetRoot(p.presets.config_root);
  setProfilesSummary(d.profiles_summary);
  setBrokenSummary(d.broken_summary);
  setPresetsSummary(d.presets_summary);
  setTemplatesSummary(d.templates_summary);
  setTemplatesIntro(d.templates_intro);
  setProfilesError(p.profiles_error ?? "");
  setPresetsError(p.presets_error ?? "");
  setMaxSlots(String(d.max_slots));
  setMaxPlayer(String(d.max_player));

  setPillRunning(d.pill_running);
  setPillIdle(d.pill_idle);
  setPillDown(d.pill_down);
  setNoDaemon(d.no_daemon);
  setCanStop(d.can_stop);
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
  setSessionLine("This screen is temporarily unavailable. Reopen ksx and try again.");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setNoDaemon(true);
  setCanStop(false);
  setRowsLive(false);
  setRowsPlain(true);
}

const FLASH_MS = 5000;
let flashTimer: ReturnType<typeof setTimeout> | undefined;

const UNKNOWN_FLASH =
  "error: Saved Games could not finish that request. Reopen ksx and try again.";
const PROFILE_FLASH_ALLOWLIST: readonly string[] = [
  "Saved game added.",
  "Saved game updated.",
  "Saved game deleted.",
  "Controller layout created.",
  "Play started.",
  "Play stopped.",
  "error: Saved game could not be added. Check the game name, program location, players, and controller layout; nothing was changed.",
  "error: Saved game could not be updated. Refresh the page, then check its details; nothing was changed.",
  "error: Saved game could not be deleted. Refresh the page and try again; nothing was changed.",
  "error: Controller layout could not be created. Choose a different name or starter layout; nothing was changed.",
  "error: That game could not be started. Open Edit and check its program and controllers.",
  "error: Play could not be stopped. Reopen ksx and try again.",
  UNKNOWN_FLASH,
  "error: that change could not be accepted. Nothing was changed. Reopen ksx and try again.",
  "error: the change could not be sent. Reopen ksx and try again.",
];

/** The payload and redirect URL are both input boundaries in the browser.
 * Accept only presentation copy owned by this screen. */
export function safeProfileFlash(flash: string | null | undefined): string {
  const candidate = (flash ?? "").trim();
  if (candidate === "") return "";
  return PROFILE_FLASH_ALLOWLIST.includes(candidate) ? candidate : UNKNOWN_FLASH;
}

export function applyFlash(flash: string | null | undefined): void {
  if (flashTimer !== undefined) {
    clearTimeout(flashTimer);
    flashTimer = undefined;
  }
  const line = safeProfileFlash(flash);
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
        h("a", { class: "navlink", href: "/start" }, "Setup"),
        h("a", { class: "navlink", href: "/map" }, "Controls"),
        h("a", { class: "navlink", href: "/check" }, "Test"),
        h("span", { class: "navlink on", "aria-current": "page" }, "Games"),
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
        () => h("span", { class: "pill pill-down" }, "needs attention"),
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
              "The background service is not responding",
            ),
            h(
              "p",
              { class: "alarmlead" },
              "You can still create or edit saved games. To play one, close ",
              "this window and reopen ksx from the desktop shortcut. If ksx is ",
              "already in the notification area, choose Open Studio there.",
            ),
            h(
              "span",
              { class: "product-hidden" },
              () => daemonCmd(),
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
            h("h2", null, "Games that need attention"),
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
                      h("span", { class: "pdetail" }, b.verdict),
                    ),
                  ),
              ),
            ),
            h(
              "p",
              { class: "cardline" },
              "Open “Edit or delete” on the affected game and correct its ",
              "program. You can also remove a saved game you no longer use.",
              h("span", { class: "product-hidden" }, () => gamesPath()),
            ),
          ),
      ),
      // ── SESSION line + flash ──────────────────────────────────────────
      h(
        "section",
        { class: "card hero session" },
        h("h2", null, "Play status"),
        h("p", { class: "state" }, () => sessionLine()),
        createShow(
          () => canStop(),
          () =>
            h(
              "form",
              { method: "post", action: "/profiles/stop" },
              h(
                "button",
                { class: "btn btn-danger-ghost", type: "submit" },
                "Stop playing",
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
      ),
      // ── PROFILES ──────────────────────────────────────────────────────
      h(
        "section",
        { class: "card wide profilecard" },
        h("h2", null, "Saved games"),
        h(
          "p",
          { class: "cardline" },
          "A saved game remembers what to launch, how many players it has, ",
          "and which controller layout they use. Choose Play game to start it now.",
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
              h("p", { class: "warn" }, "Saved games are temporarily unavailable."),
              h(
                "p",
                { class: "cardline" },
                "This is a read failure, not an empty saved-game list. Reopen ksx ",
                "and try again. Your saved games have not been replaced.",
                h("span", { class: "product-hidden" }, () => gamesPath()),
              ),
              h(
                "details",
                { class: "st-more" },
                h("summary", null, "Support details"),
                h("p", { class: "pdetail" }, () => profilesError()),
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
                (g) =>
                  g.title +
                  "|" +
                  g.revision +
                  "|" +
                  g.path +
                  "|" +
                  g.arguments +
                  "|" +
                  g.slots +
                  "|" +
                  g.max_slots +
                  "|" +
                  g.preset +
                  "|" +
                  g.detail +
                  "|" +
                  g.verdict +
                  "|" +
                  g.statecls +
                  "|" +
                  g.statelabel,
                (g) =>
                  h(
                    "li",
                    { class: "profile-row" },
                    h(
                      "div",
                      { class: "profile-row-head" },
                      h(
                        "div",
                        { class: "pmeta" },
                        h("span", { class: "ptitle" }, g.title),
                        h("span", { class: "pdetail" }, g.detail),
                        h("span", { class: "pdetail" }, g.verdict),
                      ),
                      h("span", { class: g.statecls }, g.statelabel),
                      h(
                        "form",
                        {
                          class: "profile-switch",
                          method: "post",
                          action: "/profiles/switch",
                        },
                        h("input", {
                          type: "hidden",
                          name: "profile",
                          value: g.title,
                        }),
                        h(
                          "button",
                          {
                            class: "btn btn-row",
                            type: "submit",
                            disabled: g.play_disabled,
                          },
                          "Play game",
                        ),
                      ),
                    ),
                    h(
                      "details",
                      { class: "disclosure profile-edit" },
                      h("summary", null, "Edit or delete"),
                      h(
                        "form",
                        {
                          class: "grid profile-edit-grid",
                          method: "post",
                          action: "/profiles/update",
                        },
                        h("input", {
                          type: "hidden",
                          name: "original_title",
                          value: g.title,
                        }),
                        h("input", {
                          type: "hidden",
                          name: "revision",
                          value: g.revision,
                        }),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Game name",
                          h("input", {
                            type: "text",
                            name: "title",
                            required: "",
                            value: g.title,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Program or game link",
                          h("input", {
                            type: "text",
                            name: "path",
                            required: "",
                            value: g.path,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Launch options (optional)",
                          h("input", {
                            type: "text",
                            name: "arguments",
                            value: g.arguments,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Players",
                          h("input", {
                            type: "number",
                            name: "slots",
                            min: "1",
                            max: g.max_slots,
                            required: "",
                            value: g.slots,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Controller layout for every player",
                          h(
                            "select",
                            { name: "preset", required: "" },
                            h(
                              "option",
                              {
                                value: g.preset,
                                selected: true,
                                hidden: true,
                              },
                              g.preset,
                            ),
                            createList(
                              () => presetOptions(),
                              (o) => o.value,
                              (o) => h("option", { value: o.value }, o.label),
                            ),
                          ),
                        ),
                        h(
                          "label",
                          { class: "checkline" },
                          h("input", {
                            type: "checkbox",
                            name: "rebase_devices",
                            value: "true",
                          }),
                          "Use the device choices currently saved in Setup",
                        ),
                        h(
                          "button",
                          { class: "btn btn-primary", type: "submit" },
                          "Save changes",
                        ),
                      ),
                      h(
                        "p",
                        { class: "profile-edit-note" },
                        "Saving applies the named controller layout to every ",
                        "player. Device choices stay as they are unless you ",
                        "select the checkbox.",
                      ),
                      h(
                        "form",
                        {
                          class: "profile-delete",
                          method: "post",
                          action: "/profiles/delete",
                          "data-confirm": "Delete this saved game? Controller layouts will remain.",
                        },
                        h("input", {
                          type: "hidden",
                          name: "title",
                          value: g.title,
                        }),
                        h("input", {
                          type: "hidden",
                          name: "revision",
                          value: g.revision,
                        }),
                        h(
                          "label",
                          { class: "checkline" },
                          h("input", {
                            type: "checkbox",
                            name: "confirm_delete",
                            value: "yes",
                            required: "",
                          }),
                          "I want to delete this saved game",
                        ),
                        h(
                          "button",
                          { class: "btn btn-danger-ghost", type: "submit" },
                          "Delete saved game",
                        ),
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
                (g) =>
                  g.title +
                  "|" +
                  g.revision +
                  "|" +
                  g.path +
                  "|" +
                  g.arguments +
                  "|" +
                  g.slots +
                  "|" +
                  g.max_slots +
                  "|" +
                  g.preset +
                  "|" +
                  g.detail +
                  "|" +
                  g.verdict +
                  "|" +
                  g.statecls +
                  "|" +
                  g.statelabel,
                (g) =>
                  h(
                    "li",
                    { class: "profile-row" },
                    h(
                      "div",
                      { class: "profile-row-head" },
                      h(
                        "div",
                        { class: "pmeta" },
                        h("span", { class: "ptitle" }, g.title),
                        h("span", { class: "pdetail" }, g.detail),
                        h("span", { class: "pdetail" }, g.verdict),
                      ),
                      h("span", { class: g.statecls }, g.statelabel),
                    ),
                    h(
                      "details",
                      { class: "disclosure profile-edit" },
                      h("summary", null, "Edit or delete"),
                      h(
                        "form",
                        {
                          class: "grid profile-edit-grid",
                          method: "post",
                          action: "/profiles/update",
                        },
                        h("input", {
                          type: "hidden",
                          name: "original_title",
                          value: g.title,
                        }),
                        h("input", {
                          type: "hidden",
                          name: "revision",
                          value: g.revision,
                        }),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Game name",
                          h("input", {
                            type: "text",
                            name: "title",
                            required: "",
                            value: g.title,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Program or game link",
                          h("input", {
                            type: "text",
                            name: "path",
                            required: "",
                            value: g.path,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Launch options (optional)",
                          h("input", {
                            type: "text",
                            name: "arguments",
                            value: g.arguments,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Players",
                          h("input", {
                            type: "number",
                            name: "slots",
                            min: "1",
                            max: g.max_slots,
                            required: "",
                            value: g.slots,
                          }),
                        ),
                        h(
                          "label",
                          { class: "bindlabel" },
                          "Controller layout for every player",
                          h(
                            "select",
                            { name: "preset", required: "" },
                            h(
                              "option",
                              {
                                value: g.preset,
                                selected: true,
                                hidden: true,
                              },
                              g.preset,
                            ),
                            createList(
                              () => presetOptions(),
                              (o) => o.value,
                              (o) => h("option", { value: o.value }, o.label),
                            ),
                          ),
                        ),
                        h(
                          "label",
                          { class: "checkline" },
                          h("input", {
                            type: "checkbox",
                            name: "rebase_devices",
                            value: "true",
                          }),
                          "Use the device choices currently saved in Setup",
                        ),
                        h(
                          "button",
                          { class: "btn btn-primary", type: "submit" },
                          "Save changes",
                        ),
                      ),
                      h(
                        "p",
                        { class: "profile-edit-note" },
                        "Saving applies the named controller layout to every ",
                        "player. Device choices stay as they are unless you ",
                        "select the checkbox.",
                      ),
                      h(
                        "form",
                        {
                          class: "profile-delete",
                          method: "post",
                          action: "/profiles/delete",
                          "data-confirm": "Delete this saved game? Controller layouts will remain.",
                        },
                        h("input", {
                          type: "hidden",
                          name: "title",
                          value: g.title,
                        }),
                        h("input", {
                          type: "hidden",
                          name: "revision",
                          value: g.revision,
                        }),
                        h(
                          "label",
                          { class: "checkline" },
                          h("input", {
                            type: "checkbox",
                            name: "confirm_delete",
                            value: "yes",
                            required: "",
                          }),
                          "I want to delete this saved game",
                        ),
                        h(
                          "button",
                          { class: "btn btn-danger-ghost", type: "submit" },
                          "Delete saved game",
                        ),
                      ),
                    ),
                  ),
              ),
            ),
        ),
      ),
      // ── NEW PROFILE — the thing that could not be done at all ─────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "Add a saved game"),
        h(
          "p",
          { class: "cardline" },
          "Save a game or launcher together with its player count and ",
          "controller layout. Each player inherits the matching device from ",
          "your saved Setup, so its controllers are ready. Paste the program location ",
          "exactly as Windows gives it; surrounding quotation marks are fine.",
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
                "Game name",
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
                "Program or game link",
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
                "Launch options (optional)",
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
                "Players",
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
                "Controller layout for every player",
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
                "Save game",
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
                "There are no controller layouts yet. Make one from a starter ",
                "layout below first — the ",
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
              h("p", { class: "warn" }, "Controller layouts are temporarily unavailable."),
              h(
                "p",
                { class: "cardline" },
                "This is a read failure, not an empty layout list. Reopen ksx ",
                "and try again. Creation stays unavailable until the saved ",
                "layouts can be read safely.",
              ),
              h(
                "details",
                { class: "st-more" },
                h("summary", null, "Support details"),
                h("p", { class: "pdetail" }, () => presetsError()),
              ),
            ),
        ),
      ),
      // ── PRESETS ───────────────────────────────────────────────────────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "Controller layouts"),
        h(
          "p",
          { class: "cardline" },
          "A controller layout is one virtual controller's button map. You can ",
          "edit any layout from Controls. Built-in layouts stay protected, and ",
          "creating a new one never overwrites an existing layout.",
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
        h("h2", null, "New controller layout"),
        // Served concise intro. The complete roster remains derived from the
        // same payload and is available in the comparison disclosure below.
        h("p", { class: "cardline" }, () => templatesIntro()),
        h("p", { class: "cardline mono" }, () => templatesSummary()),
        // `TemplateRow.detail` is the panel note that lets a person compare
        // layouts. Keep the complete list available without making everybody
        // read six long descriptions before reaching the form.
        h(
          "details",
          { class: "st-more" },
          h("summary", null, "Compare starter layouts"),
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
                    h("span", { class: "ptitle" }, t.label),
                    h("span", { class: "pdetail" }, t.detail),
                  ),
                  h("span", { class: "pill pill-idle" }, t.players),
                ),
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
                "Name for the new layout",
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
                "Starter layout",
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
                "Which player's keys to copy — use the range shown in Starter layout",
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
                "Create layout",
              ),
            ),
        ),
        h(
          "p",
          { class: "cardline" },
          h("span", { class: "product-hidden" }, () => presetRoot()),
        ),
      ),
      // ── NOTES: anything the reads had to say out loud ─────────────────
      createShow(
        () => anyNotes(),
        () =>
          h(
            "section",
            { class: "card" },
            h(
              "details",
              { class: "st-more" },
              h("summary", null, "Support details"),
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
    ),
    h(
      "footer",
      null,
      h(
        "p",
        null,
        "This page stays up to date while it is open. Last checked ",
        h("span", { class: "mono" }, () => generatedAt()),
        ".",
      ),
    ),
  );
}
