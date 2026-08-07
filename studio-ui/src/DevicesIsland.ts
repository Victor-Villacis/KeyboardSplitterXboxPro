import { h, createSignal, createList, createShow } from "@getforma/core";

// The `/devices` island: which boards are plugged in, which one ksx is
// configured to use, and the two writes that change that.
//
// # What this page is, and the three things it is not
//
// It is the web face of `ksx device scan` (read) plus `ksx device pick` and
// `ksx device remove` (write) — `docs/SURFACES.md` §3 puts "device pick /
// remove" on Studio, following the CLI verb. Every button here is one
// `MachineSource` call; nothing below decides anything.
//
// It is NOT a claim screen. Taking a board off the Windows keyboard stack
// needs an elevated process, so §3 marks WinUSB claim/release "never" for the
// browser. The commands are SHOWN, as copyable text, and that is the whole of
// it — a button that could only ever fail is worse than no button.
//
// It is NOT the other two removals. ksx has three and they are routinely
// confused: `ksx pads --prune` drops stale VIRTUAL PADS off the ViGEm bus,
// `ksx winusb release` puts a claimed board BACK on the keyboard driver, and
// the Remove button here deletes a `[[device]]` entry from config.toml.
// Deleting the entry releases nothing. The card at the foot of the page says
// so, and the remove outcome says so again when the board really is claimed.
//
// # Compiler constraints (identical to StatusIsland.ts — read them there)
//
// - dynamic text/attrs are bare `() => signalName()` calls; the slot is named
//   after the getter;
// - list sources are bare `() => listSignal()` calls;
// - list item bodies may only use direct member reads (`r.alias`);
// - createShow conditions are bare `() => signalName()` calls;
// - no string concatenation anywhere in the tree. A `+` compiles to an
//   ANONYMOUS slot the server can never inject (render.rs ledger #10/#20), so
//   every composed sentence is composed in `applyDevices` below and in
//   `render_devices.rs`, and shipped as a signal value. `ANONYMOUS_SLOTS` on
//   the Rust side is asserted EMPTY for this page.
// - optional lines are never conditionally built. A row carries its own class
//   (`r.portWarnCls` = `"dv-warn"` or `"dv-warn dv-hide"`), because a
//   `createShow` inside a `createList` is not a shape this compiler emits.

// ── Wire types: serde field names from crates/ksx-api/src/machine.rs ───────

export interface UsbRow {
  instance_id: string;
  description: string;
  state: string;
  verdict: string;
  alias: string | null;
  selected: boolean;
  ready: boolean;
  vendor?: string | null;
  board?: string | null;
  boot_keyboard: boolean;
}

export interface BoardRow {
  name: string;
  interfaces: UsbRow[];
  keyboard: string | null;
  keyboard_verdict: string;
  looks_like_a_keyboard: boolean;
  claimed: boolean;
  alias: string | null;
  claim_command: string | null;
  release_command: string | null;
}

export interface ConfiguredDevice {
  alias: string;
  id: string;
  backend: string;
  rung: string;
  survives_replug: boolean;
  means: string;
  port_pinned_warning: string | null;
  present: boolean;
  board: string | null;
  instance_id: string | null;
  claimed: boolean;
  claim_command: string | null;
  release_command: string | null;
  used_by: string[];
}

export interface DeviceScanView {
  generated_at: string;
  usb_available: boolean;
  boards: BoardRow[];
  configured: ConfiguredDevice[];
  notes: string[];
}

export interface SessionView {
  reachable: boolean;
  running: boolean;
  line: string;
  profile: string | null;
}

/** What GET /api/devices serves and what the island props carry — one shape
 *  (`DevicesPayload` in snapshot.rs; parity unit-tested in render_devices.rs). */
export interface DevicesPayload {
  scan: DeviceScanView;
  session: SessionView;
  unavailable: string;
  flash: string | null;
}

// ── Row shapes: every string decided server-side, mirrored here ────────────

interface ConfiguredTile {
  alias: string;
  id: string;
  backend: string;
  means: string;
  presence: string;
  presenceCls: string;
  claimText: string;
  claimCls: string;
  board: string;
  boardCls: string;
  commandLead: string;
  command: string;
  commandCls: string;
  portWarn: string;
  portWarnCls: string;
  usedBy: string;
  usedByCls: string;
  forceId: string;
  forceCls: string;
}

interface BoardTile {
  name: string;
  ifaces: string;
  verdict: string;
  caveat: string;
  caveatCls: string;
  configured: string;
  configuredCls: string;
  claimText: string;
  claimCls: string;
  commandLead: string;
  command: string;
  commandCls: string;
  query: string;
  aliasId: string;
  aliasHint: string;
  pickLabel: string;
}

interface OtherTile {
  name: string;
  ifaces: string;
}

interface NoteTile {
  note: string;
}

// ── The live state store (module-level: one island, page lifetime) ─────────

const [generatedAt, setGeneratedAt] = createSignal("(no scan)");
const [sessionLine, setSessionLine] = createSignal("not collected");
const [flashLine, setFlashLine] = createSignal("");
const [unavailableLine, setUnavailableLine] = createSignal("");
const [configuredSummary, setConfiguredSummary] = createSignal("not collected");
const [boardsSummary, setBoardsSummary] = createSignal("not collected");
const [otherSummary, setOtherSummary] = createSignal("");

const [pillRunning, setPillRunning] = createSignal(false);
const [pillIdle, setPillIdle] = createSignal(false);
const [pillDown, setPillDown] = createSignal(false);
const [showUnavailable, setShowUnavailable] = createSignal(false);
const [flashOk, setFlashOk] = createSignal(false);
const [flashError, setFlashError] = createSignal(false);
const [sessionLive, setSessionLive] = createSignal(false);
const [hasConfigured, setHasConfigured] = createSignal(false);
const [noConfigured, setNoConfigured] = createSignal(false);
const [hasBoards, setHasBoards] = createSignal(false);
const [noBoards, setNoBoards] = createSignal(false);
const [hasOther, setHasOther] = createSignal(false);
const [hasNotes, setHasNotes] = createSignal(false);

const [configuredRows, setConfiguredRows] = createSignal<ConfiguredTile[]>([]);
const [boardRows, setBoardRows] = createSignal<BoardTile[]>([]);
const [otherRows, setOtherRows] = createSignal<OtherTile[]>([]);
const [noteRows, setNoteRows] = createSignal<NoteTile[]>([]);

// ── Derivations (mirror render_devices.rs; pinned there by unit tests) ─────

/** The elevated half, said the same way in both places. Claiming and
 *  releasing need an admin shell, so what a browser can do with either is show
 *  it — and the sentence has to say WHY, or a copyable command that fails with
 *  "access denied" is the only feedback the user ever gets. */
const CLAIM_LEAD =
  "To move it to the WinUSB backend it must be claimed — it then stops typing to Windows, which is a separate, consented step. Run this in an ELEVATED shell:";
const RELEASE_LEAD =
  "It is claimed, so Windows sees no keyboard on it. To put it back on the keyboard driver, run this in an ELEVATED shell:";

function configuredSummaryLine(count: number): string {
  if (count === 0) return "no [[device]] entries in config.toml";
  if (count === 1) return "1 [[device]] entry in config.toml:";
  return `${count} [[device]] entries in config.toml:`;
}

function boardsSummaryLine(count: number, usbAvailable: boolean): string {
  if (!usbAvailable) {
    return "USB enumeration failed — this list is empty because nothing could be READ, not because nothing is plugged in";
  }
  if (count === 0) return "no keyboard-capable board found";
  if (count === 1) return "1 keyboard-capable board:";
  return `${count} keyboard-capable boards:`;
}

function otherSummaryLine(count: number): string {
  if (count === 0) return "";
  if (count === 1) {
    return "1 board has no keyboard interface — ksx cannot capture it:";
  }
  return `${count} boards have no keyboard interface — ksx cannot capture them:`;
}

function configuredTile(d: ConfiguredDevice, i: number): ConfiguredTile {
  const present = d.present;
  const winusbButLoose = present && !d.claimed && d.backend === "winusb";
  let claimText = "";
  let claimCls = "pill dv-hide";
  if (present && d.claimed) {
    claimText = "claimed — bound to winusb.sys";
    claimCls = "pill pill-ok";
  } else if (winusbButLoose) {
    claimText = "backend is winusb but the board is NOT claimed — ksx run will refuse";
    claimCls = "pill pill-warn";
  } else if (present) {
    claimText = "on the Windows keyboard stack";
    claimCls = "pill pill-idle";
  }

  let commandLead = "";
  let command = "";
  let commandCls = "dv-cmd dv-hide";
  if (d.release_command) {
    commandLead = RELEASE_LEAD;
    command = d.release_command;
    commandCls = "dv-cmd";
  } else if (d.claim_command) {
    commandLead = CLAIM_LEAD;
    command = d.claim_command;
    commandCls = "dv-cmd";
  }

  return {
    alias: d.alias,
    id: d.id,
    backend: d.backend,
    means: d.means,
    presence: present ? "connected" : "not connected right now",
    presenceCls: present ? "pill pill-ok" : "pill pill-warn",
    claimText,
    claimCls,
    board:
      present && d.instance_id
        ? `${d.board ?? "unknown board"} — ${d.instance_id}`
        : "the id resolves to no connected interface right now — unplugged, moved to another socket, or never here",
    boardCls: present ? "dv-line mono" : "dv-line dv-miss",
    commandLead,
    command,
    commandCls,
    portWarn: d.port_pinned_warning ?? "",
    portWarnCls: d.port_pinned_warning ? "dv-warn" : "dv-warn dv-hide",
    usedBy:
      d.used_by.length === 0
        ? ""
        : `slots naming it: ${d.used_by.join(", ")} — removing it breaks them, so it needs the box below`,
    usedByCls: d.used_by.length === 0 ? "dv-used dv-hide" : "dv-used",
    forceId: `dv-force-${i}`,
    forceCls: d.used_by.length === 0 ? "dv-force dv-hide" : "dv-force",
  };
}

function boardTile(b: BoardRow, i: number): BoardTile {
  const keyboard = b.keyboard ?? "";
  let commandLead = "";
  let command = "";
  let commandCls = "dv-cmd dv-hide";
  if (b.release_command) {
    commandLead = RELEASE_LEAD;
    command = b.release_command;
    commandCls = "dv-cmd";
  } else if (b.claim_command) {
    commandLead = CLAIM_LEAD;
    command = b.claim_command;
    commandCls = "dv-cmd";
  }
  return {
    name: b.name,
    ifaces: `${b.interfaces.length} interface(s) · keyboard on ${keyboard}`,
    verdict: b.keyboard_verdict,
    caveat: b.looks_like_a_keyboard
      ? ""
      : "NOT declared as a keyboard. This is an HID interface and may be something else entirely — on a real cabinet a mouse, an LED controller and a fan controller all read as claimable.",
    caveatCls: b.looks_like_a_keyboard ? "dv-warn dv-hide" : "dv-warn",
    configured: b.alias ? `configured as "${b.alias}"` : "",
    configuredCls: b.alias ? "pill pill-ok" : "pill dv-hide",
    claimText: b.claimed ? "claimed — bound to winusb.sys" : "on the Windows keyboard stack",
    claimCls: b.claimed ? "pill pill-ok" : "pill pill-idle",
    commandLead,
    command,
    commandCls,
    query: keyboard,
    aliasId: `dv-alias-${i}`,
    aliasHint: b.alias ?? b.name,
    pickLabel: b.alias ? "Re-pick — update this entry" : "Use this board",
  };
}

/** Write one /api/devices payload into every signal (flash excluded — flash is
 *  one-shot action feedback, owned by `applyFlash`). Safe to call before
 *  adoption AND per poll. */
export function applyDevices(p: DevicesPayload): void {
  const scan = p.scan;
  const session = p.session;
  const unavailable = (p.unavailable ?? "").trim();

  setGeneratedAt(scan.generated_at);
  setSessionLine(session.line);
  setUnavailableLine(unavailable);
  setShowUnavailable(unavailable !== "");

  setPillRunning(session.reachable && session.running);
  setPillIdle(session.reachable && !session.running);
  setPillDown(!session.reachable);
  setSessionLive(session.reachable && session.running);

  const configured = scan.configured.map(configuredTile);
  // Pickable = it has a keyboard interface. A board with none cannot be
  // picked, so offering it would be an offer that always refuses — it goes in
  // the quiet list below instead of vanishing, because "ksx cannot see my
  // board" is a real support question.
  const pickable = scan.boards.filter((b) => b.keyboard !== null);
  const other = scan.boards.filter((b) => b.keyboard === null);

  setConfiguredRows(configured);
  setBoardRows(pickable.map(boardTile));
  setOtherRows(
    other.map((b) => ({
      name: b.name,
      ifaces: `${b.interfaces.length} interface(s) · no keyboard interface`,
    })),
  );
  setNoteRows(scan.notes.map((note) => ({ note })));

  setConfiguredSummary(configuredSummaryLine(configured.length));
  setBoardsSummary(boardsSummaryLine(pickable.length, scan.usb_available));
  setOtherSummary(otherSummaryLine(other.length));

  setHasConfigured(configured.length > 0);
  setNoConfigured(configured.length === 0);
  setHasBoards(pickable.length > 0);
  setNoBoards(pickable.length === 0 && unavailable === "");
  setHasOther(other.length > 0);
  setHasNotes(scan.notes.length > 0);
}

/** The studio server itself stopped answering: say so, and leave the last-known
 *  lists on screen. Their timestamp stops advancing, which is the honest tell —
 *  the same contract StatusIsland's `applyUnreachable` keeps. */
export function applyUnreachable(): void {
  setSessionLine("ksx-studio not responding — retrying every 2 s");
  setPillRunning(false);
  setPillIdle(false);
  setPillDown(true);
  setSessionLive(false);
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

export function DevicesIsland() {
  return h(
    "div",
    { class: "studio devices" },
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
          { class: "navlink on", href: "/devices", "aria-current": "page" },
          "Devices",
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
      // The scan itself refused. Not an empty list — an empty list on a
      // machine with four boards plugged in is the worst lie this page could
      // tell, so the two states are drawn differently and never conflated.
      createShow(
        () => showUnavailable(),
        () =>
          h(
            "section",
            { class: "card alarm" },
            h("h2", null, "This machine's devices could not be read."),
            h("p", { class: "alarmlead" }, () => unavailableLine()),
            h(
              "p",
              { class: "alarmlead" },
              "Nothing below is a reading of your hardware. The CLI verb still ",
              "works everywhere Studio does not:",
            ),
            h("p", null, h("code", { class: "mono copyable" }, "ksx device scan")),
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
      // ── CONFIGURED: what config.toml already names ────────────────────
      h(
        "section",
        { class: "card wide dv-card" },
        h("h2", null, "Configured devices"),
        h(
          "p",
          { class: "cardline" },
          "The [[device]] entries in config.toml — the names your [[slot]] ",
          "entries point at. Removing one here deletes those four lines of ",
          "TOML and nothing else: it does not release a claimed board, and it ",
          "does not touch the virtual pads on the ViGEm bus.",
        ),
        h("p", { class: "cardline mono" }, () => configuredSummary()),
        createShow(
          () => sessionLive(),
          () =>
            h(
              "p",
              { class: "warn" },
              "A session is running. These buttons write config.toml; the ",
              "running session keeps the devices it already opened until it is ",
              "stopped and started again.",
            ),
        ),
        createShow(
          () => hasConfigured(),
          () =>
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => configuredRows(),
                (r) => r.alias + "|" + r.id + "|" + r.presence + "|" + r.claimText,
                (r) =>
                  h(
                    "li",
                    { class: "dv-row" },
                    h(
                      "div",
                      { class: "dv-head" },
                      h("span", { class: "dv-name" }, r.alias),
                      h("span", { class: r.presenceCls }, r.presence),
                      h("span", { class: r.claimCls }, r.claimText),
                    ),
                    h("p", { class: "dv-line mono" }, r.id),
                    h("p", { class: "dv-note" }, r.means),
                    h("p", { class: r.boardCls }, r.board),
                    h("p", { class: r.portWarnCls }, r.portWarn),
                    h("p", { class: r.usedByCls }, r.usedBy),
                    h(
                      "div",
                      { class: r.commandCls },
                      h("span", { class: "dv-cmdlead" }, r.commandLead),
                      h("code", { class: "mono copyable" }, r.command),
                    ),
                    h(
                      "form",
                      { class: "dv-form", method: "post", action: "/devices/remove" },
                      h("input", { type: "hidden", name: "alias", value: r.alias }),
                      h(
                        "span",
                        { class: r.forceCls },
                        h("input", {
                          type: "checkbox",
                          id: r.forceId,
                          name: "force",
                          value: "yes",
                        }),
                        h(
                          "label",
                          { for: r.forceId },
                          "remove it anyway, and leave those slots naming nothing",
                        ),
                      ),
                      h(
                        "button",
                        { class: "btn btn-danger btn-row", type: "submit" },
                        "Remove entry",
                      ),
                    ),
                  ),
              ),
            ),
        ),
        createShow(
          () => noConfigured(),
          () =>
            h(
              "p",
              { class: "dv-empty" },
              "No board is configured yet, so no [[slot]] can name one. Pick ",
              "one from the list below — it writes the entry and stops there.",
            ),
        ),
      ),
      // ── BOARDS: the picker ────────────────────────────────────────────
      h(
        "section",
        { class: "card wide dv-card" },
        h("h2", null, "Boards found"),
        h(
          "p",
          { class: "cardline" },
          "One row per PHYSICAL board, not per devnode: an I-PAC is one device ",
          "to you and three interfaces to Windows, and picking the wrong one of ",
          "the three is how a slot ends up silently never firing. Picking writes ",
          "the [[device]] entry. It never claims — a claim takes the board off ",
          "the Windows keyboard stack and needs an elevated shell, so this page ",
          "shows that command instead of running it.",
        ),
        h("p", { class: "cardline mono" }, () => boardsSummary()),
        createShow(
          () => hasBoards(),
          () =>
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => boardRows(),
                (b) => b.name + "|" + b.query + "|" + b.claimText + "|" + b.configured,
                (b) =>
                  h(
                    "li",
                    { class: "dv-row" },
                    h(
                      "div",
                      { class: "dv-head" },
                      h("span", { class: "dv-name" }, b.name),
                      h("span", { class: b.configuredCls }, b.configured),
                      h("span", { class: b.claimCls }, b.claimText),
                    ),
                    h("p", { class: "dv-line mono" }, b.ifaces),
                    h("p", { class: "dv-note" }, b.verdict),
                    h("p", { class: b.caveatCls }, b.caveat),
                    h(
                      "div",
                      { class: b.commandCls },
                      h("span", { class: "dv-cmdlead" }, b.commandLead),
                      h("code", { class: "mono copyable" }, b.command),
                    ),
                    h(
                      "form",
                      { class: "dv-form", method: "post", action: "/devices/pick" },
                      h("input", { type: "hidden", name: "query", value: b.query }),
                      h("label", { class: "dv-lbl", for: b.aliasId }, "name it"),
                      h("input", {
                        class: "dv-alias",
                        type: "text",
                        id: b.aliasId,
                        name: "alias",
                        placeholder: b.aliasHint,
                        maxlength: "64",
                        autocomplete: "off",
                      }),
                      h(
                        "button",
                        { class: "btn btn-primary btn-row", type: "submit" },
                        b.pickLabel,
                      ),
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
              { class: "dv-empty" },
              "No board here exposes a keyboard interface, so there is nothing ",
              "ksx could capture. Everything that IS plugged in is listed below.",
            ),
        ),
      ),
      // ── THE REST: listed, never hidden ────────────────────────────────
      createShow(
        () => hasOther(),
        () =>
          h(
            "section",
            { class: "card dv-card" },
            h("h2", null, "Other USB interfaces"),
            h(
              "p",
              { class: "cardline" },
              "Here because \"ksx cannot see my board\" is a real question and ",
              "the answer is sometimes \"it is there, it is just not a keyboard\".",
            ),
            h("p", { class: "cardline mono" }, () => otherSummary()),
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => otherRows(),
                (o) => o.name + "|" + o.ifaces,
                (o) =>
                  h(
                    "li",
                    { class: "dv-row quiet" },
                    h("span", { class: "dv-name" }, o.name),
                    h("span", { class: "dv-line mono" }, o.ifaces),
                  ),
              ),
            ),
          ),
      ),
      createShow(
        () => hasNotes(),
        () =>
          h(
            "section",
            { class: "card dv-card" },
            h("h2", null, "Notes from the enumeration"),
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => noteRows(),
                (n) => n.note,
                (n) => h("li", { class: "dv-row quiet" }, n.note),
              ),
            ),
          ),
      ),
      // ── The disambiguation, permanently on the page ───────────────────
      // Three removals exist and they are routinely confused. Writing this out
      // once, where the Remove button is, is cheaper than the support question
      // that follows someone deleting an entry and wondering why their panel
      // still does not type.
      h(
        "section",
        { class: "card dv-card dv-legend" },
        h("h2", null, "Three different removals"),
        h(
          "ul",
          { class: "alarmways" },
          h(
            "li",
            null,
            "Remove entry (this page) — deletes a [[device]] line from ",
            "config.toml. The board keeps whatever driver it has.",
          ),
          h(
            "li",
            null,
            "ksx winusb release <ID> --yes — puts a CLAIMED board back on the ",
            "Windows keyboard driver, so it types again. Needs an elevated shell.",
          ),
          h(
            "li",
            null,
            "ksx pads --prune — drops stale VIRTUAL PADS off the ViGEm bus. ",
            "Nothing to do with keyboards at all. Needs an elevated shell.",
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
        "Read fresh every 2 s; nothing on this page is cached. Picking and ",
        "removing write config.toml through the same planner `ksx device pick` ",
        "and `ksx device remove` use, and take a timestamped backup first. ",
        "Enumerated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Session: ",
        h("span", { class: "mono" }, () => sessionLine()),
        ". Serving 127.0.0.1 only.",
      ),
    ),
  );
}
