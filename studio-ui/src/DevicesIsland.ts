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
  /** `usb` | `bluetooth`. Never inferred from anything else on the row. */
  transport: string;
  state: string;
  verdict: string;
  alias: string | null;
  selected: boolean;
  ready: boolean;
  vendor?: string | null;
  board?: string | null;
  boot_keyboard: boolean;
  interception_eligible: boolean;
  winusb_eligible: boolean;
  backends: string;
  can_type: boolean;
  cannot_type_reason: string;
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
  /** Decided by DeviceScanView::read — see the DERIVED note below. */
  pickable: boolean;
  command_lead: string;
  command: string;
  caveat: string;
  /** `usb` | `bluetooth` — decided by DeviceScanView::read from the interface
   *  a pick would name. */
  transport: string;
  /** `USB` | `Bluetooth`, as a human reads it. SERVED, not mapped here: a
   *  three-line mapping is exactly the size of thing that gets written twice
   *  and then disagrees, and a device labelled two ways is the confusion this
   *  column exists to remove. */
  transport_label: string;
  interception_eligible: boolean;
  winusb_eligible: boolean;
  /** Which backends can reach it, and with what caveat. Composed by
   *  ksx_core::Reach — the rule is NOT derivable from `state`. */
  backends: string;
  can_type: boolean;
  cannot_type_reason: string;
  /** The whole "present but cannot type" caveat, or "". Composed by
   *  DeviceScanView::read, which also decides that a CLAIMED board does not
   *  get it. */
  cannot_type_line: string;
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
  /** `usb` | `bluetooth` for the device this entry resolved to; "" while it
   *  resolves to nothing. `backend = "winusb"` on a bluetooth one is a config
   *  that can NEVER work, which is why health_line below reads it. */
  transport: string;
  claimed: boolean;
  claim_command: string | null;
  release_command: string | null;
  used_by: string[];
  /** Decided by DeviceScanView::read — see the DERIVED note below. */
  health_line: string;
  /** `ok` | `warn` | `idle` | `none`. */
  health_level: string;
  command_lead: string;
  command: string;
}

export interface DeviceScanView {
  generated_at: string;
  usb_available: boolean;
  /** A SEPARATE read from usb_available, and either can fail alone. Collapsing
   *  them lets a dead Bluetooth walk hide behind a healthy USB one and print a
   *  list that silently omits half the machine. Nothing here reads it directly
   *  — boards_summary and no_pickable_board_found already account for it. */
  bluetooth_available: boolean;
  boards: BoardRow[];
  configured: ConfiguredDevice[];
  notes: string[];
  // ── DERIVED: decided once, in Rust (crates/ksx-api/src/machine.rs) ──────
  //
  // This island is the SECOND renderer of this payload — `render_devices.rs`
  // paints the same page server-side — so anything decided here is decided
  // twice, in two languages, and drifts silently. It did: the partition, the
  // counts and these three sentences were computed in both files, and only the
  // Rust copy consulted `usb_available`, so a machine whose USB enumeration
  // FAILED was told "no keyboard-capable board found".
  //
  // Nothing below is recomputed here. `DeviceScanView::read` decides it and
  // this file renders it. Do not add a derivation to this island; add it there.
  pickable_boards: number;
  other_boards: number;
  configured_summary: string;
  boards_summary: string;
  other_summary: string;
  /** The list is empty AND that emptiness is a real reading of the machine.
   *  False whenever nothing could be read — "I could not read this" and "there
   *  is nothing here" are different sentences and the user acts on them
   *  differently. */
  no_pickable_board_found: boolean;
  /** The same, for config.toml. */
  no_configured_device: boolean;
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
  rung: string;
  transport: string;
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
  transport: string;
  backends: string;
  cantType: string;
  cantTypeCls: string;
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
  transport: string;
  ifaces: string;
  /** Why no backend can reach it. On Bluetooth that answer is PERMANENT, and
   *  "ksx cannot see my device" is answered differently depending on which. */
  backends: string;
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

// ── Row shaping (mirrors render_devices.rs; NOT deciding anything) ─────────
//
// Everything below turns a value the backend already decided into the class or
// element id this page draws it with. No sentence is composed here, no count is
// taken here, and no verdict is reached here — see the DERIVED note on
// DeviceScanView for what that cost the last time.

/** A line that is present on every row and hidden when it has nothing to say:
 *  `createShow` inside a `createList` is not a shape this compiler emits. */
function optionalLine(text: string, cls: string): [string, string] {
  return text === "" ? ["", `${cls} dv-hide`] : [text, cls];
}

/** `pill pill-<level>` — the level word travels from the backend, so a level it
 *  adds cannot leave this page rendering the wrong colour. `pill-none` is the
 *  hidden one (studio.css). */
function pillOf(level: string): string {
  return `pill pill-${level}`;
}

function configuredTile(d: ConfiguredDevice, i: number): ConfiguredTile {
  const present = d.present;
  const [commandLead, commandCls] = optionalLine(d.command_lead, "dv-cmd");
  const [portWarn, portWarnCls] = optionalLine(d.port_pinned_warning ?? "", "dv-warn");
  const used = d.used_by.length > 0;
  return {
    alias: d.alias,
    id: d.id,
    backend: d.backend,
    rung: d.rung,
    // "" while the entry resolves to nothing: a transport is a fact about a
    // device that is HERE, and a guess from the id's spelling would be a claim
    // about hardware nobody found.
    transport: d.transport,
    means: d.means,
    presence: present ? "connected" : "not connected right now",
    presenceCls: present ? "pill pill-ok" : "pill pill-warn",
    claimText: d.health_line,
    claimCls: pillOf(d.health_level),
    board:
      present && d.instance_id
        ? `${d.board ?? "unknown board"} — ${d.instance_id}`
        : "the id resolves to no connected interface right now — unplugged, moved to another socket, or never here",
    boardCls: present ? "dv-line mono" : "dv-line dv-miss",
    commandLead,
    command: d.command,
    commandCls,
    portWarn,
    portWarnCls,
    usedBy: used
      ? `slots naming it: ${d.used_by.join(", ")} — removing it breaks them, so it needs the box below`
      : "",
    usedByCls: used ? "dv-used" : "dv-used dv-hide",
    forceId: `dv-force-${i}`,
    forceCls: used ? "dv-force" : "dv-force dv-hide",
  };
}

function boardTile(b: BoardRow, i: number): BoardTile {
  const keyboard = b.keyboard ?? "";
  const [commandLead, commandCls] = optionalLine(b.command_lead, "dv-cmd");
  const [caveat, caveatCls] = optionalLine(b.caveat, "dv-warn");
  // Both SERVED. The transport word decides whether the WinUSB half of the
  // backends line is "after a claim" or "never", so a page that mapped either
  // itself would be re-deciding the one rule this column exists to state.
  const [cantType, cantTypeCls] = optionalLine(b.cannot_type_line, "dv-warn");
  return {
    name: b.name,
    transport: b.transport_label,
    backends: b.backends,
    cantType,
    cantTypeCls,
    ifaces: `${b.interfaces.length} interface(s) · keyboard on ${keyboard}`,
    verdict: b.keyboard_verdict,
    caveat,
    caveatCls,
    configured: b.alias ? `configured as "${b.alias}"` : "",
    configuredCls: b.alias ? "pill pill-ok" : "pill dv-hide",
    claimText: b.claimed ? "claimed — bound to winusb.sys" : "on the Windows keyboard stack",
    claimCls: b.claimed ? "pill pill-ok" : "pill pill-idle",
    commandLead,
    command: b.command,
    commandCls,
    query: keyboard,
    aliasId: `dv-alias-${i}`,
    // The box is PRE-FILLED with the name the entry already has, not merely
    // placeholdered with it. A placeholder reads as "already filled in" and
    // typing over it renamed the entry, orphaning every [[slot]] that named the
    // old alias — the same destruction the Remove button below demands a
    // --force checkbox for. (`plan_pick` now refuses that rename outright; this
    // is the half that stops it being reached by accident.)
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

  // `b.pickable`, never `b.keyboard !== null`. A board with no keyboard
  // interface cannot be picked, so offering it would be an offer that always
  // refuses — it goes in the quiet list below instead of vanishing, because
  // "ksx cannot see my board" is a real support question. Which side a board
  // falls on is DeviceScanView::read's decision; this only draws it.
  setConfiguredRows(scan.configured.map(configuredTile));
  setBoardRows(scan.boards.filter((b) => b.pickable).map(boardTile));
  setOtherRows(
    scan.boards
      .filter((b) => !b.pickable)
      .map((b) => ({
        name: b.name,
        transport: b.transport_label,
        ifaces: `${b.interfaces.length} interface(s) · no keyboard interface`,
        backends: b.backends,
      })),
  );
  setNoteRows(scan.notes.map((note) => ({ note })));

  setConfiguredSummary(scan.configured_summary);
  setBoardsSummary(scan.boards_summary);
  setOtherSummary(scan.other_summary);

  setHasConfigured(scan.configured.length > 0);
  setHasBoards(scan.pickable_boards > 0);
  setHasOther(scan.other_boards > 0);
  setHasNotes(scan.notes.length > 0);
  // The two empty-state paragraphs are gated on the backend's flags, NOT on
  // `length === 0`. A refusal or a failed enumeration arrives with empty lists,
  // and both flags false, so neither paragraph appears and the banner above is
  // the only thing that speaks. This island previously wrote
  // `pickable.length === 0 && unavailable === ""`, which forgot `usb_available`
  // entirely and told a cabinet with four boards plugged in that it had none.
  setNoConfigured(scan.no_configured_device);
  setNoBoards(scan.no_pickable_board_found);
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
                    // backend and rung, both RENDERED. The page used to carry
                    // `backend` in the row object and read it nowhere, so it
                    // never said whether an entry was winusb or interception —
                    // the field the pill above is reasoning about — and `rung`
                    // was not carried at all. Static labels beside dynamic
                    // values: no concatenation, so no anonymous slot.
                    h(
                      "p",
                      { class: "dv-facts" },
                      h("span", { class: "dv-lbl" }, "backend"),
                      h("span", { class: "mono" }, r.backend),
                      h("span", { class: "dv-lbl" }, "rung"),
                      h("span", { class: "mono" }, r.rung),
                      // The third fact the pill above is reasoning about:
                      // `winusb` on a bluetooth transport is not a claim
                      // somebody forgot, it is one nobody can perform.
                      h("span", { class: "dv-lbl" }, "transport"),
                      h("span", { class: "mono" }, r.transport),
                    ),
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
        h("h2", null, "Devices found"),
        h(
          "p",
          { class: "cardline" },
          "One row per PHYSICAL device, not per devnode: an I-PAC is one device ",
          "to you and three interfaces to Windows, and picking the wrong one of ",
          "the three is how a slot ends up silently never firing. Picking writes ",
          "the [[device]] entry. It never claims — a claim takes the board off ",
          "the Windows keyboard stack and needs an elevated shell, so this page ",
          "shows that command instead of running it.",
        ),
        h(
          "p",
          { class: "cardline" },
          "USB and Bluetooth are in one list, and each row says which backends ",
          "can reach it. A Bluetooth keyboard is capturable TODAY through ",
          "Interception — it is a keyboard on the Windows input stack like any ",
          "other, so ksx can split it into virtual pads. It can never be ",
          "WinUSB-claimed: a claim binds a USB interface through an INF hardware ",
          "id, and a Bluetooth device has none. That is the transport, not a ",
          "feature waiting to be written.",
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
                      // The transport, on the head line, because it decides
                      // everything below it — and on EVERY row, not only the
                      // surprising one: a rule stated where it bites reads as
                      // a special case rather than as the rule.
                      h("span", { class: "pill pill-idle" }, b.transport),
                      h("span", { class: b.configuredCls }, b.configured),
                      h("span", { class: b.claimCls }, b.claimText),
                    ),
                    h("p", { class: "dv-line mono" }, b.ifaces),
                    h("p", { class: "dv-note" }, b.verdict),
                    // Which backends can reach this device, and why the ones
                    // that cannot, cannot. Composed by ksx_core::Reach — the
                    // answer is not derivable from anything else on the row,
                    // which is exactly why it is a line and not an inference.
                    h("p", { class: "dv-note" }, b.backends),
                    h("p", { class: b.cantTypeCls }, b.cantType),
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
                        value: b.aliasHint,
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
              // Only ever shown when the enumeration ANSWERED — noBoards is
              // `scan.no_pickable_board_found`, which is false whenever
              // nothing could be read. The first sentence says so out loud,
              // because that is the difference between this paragraph and the
              // banner above it.
              "The enumeration answered, and no board it found exposes a ",
              "keyboard interface — so there is nothing here ksx could ",
              "capture. Anything else that enumerated is listed below.",
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
            h("h2", null, "Other devices"),
            h(
              "p",
              { class: "cardline" },
              "Here because \"ksx cannot see my device\" is a real question and ",
              "the answer is sometimes \"it is there, it is just not a keyboard\". ",
              "Both transports are in this list: a Bluetooth speaker and a USB ",
              "hub are equally not keyboards, and each row says which it is.",
            ),
            h("p", { class: "cardline mono" }, () => otherSummary()),
            h(
              "ul",
              { class: "plist dv-list" },
              createList(
                () => otherRows(),
                (o) => o.name + "|" + o.transport + "|" + o.ifaces,
                (o) =>
                  h(
                    "li",
                    { class: "dv-row quiet" },
                    h("span", { class: "dv-name" }, o.name),
                    h("span", { class: "pill pill-idle" }, o.transport),
                    h("span", { class: "dv-line mono" }, o.ifaces),
                    h("span", { class: "dv-note" }, o.backends),
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
