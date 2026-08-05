import { h, createSignal, createList, createShow } from "@getforma/core";

// The cabinet control room — SSR-only, zero client JS, one screen.
//
// Design: dark "steel" surface, one teal accent, PadForge-informed structure
// (hero session card, slot-tile grid with hand-drawn gamepad silhouettes,
// micro-label card headers, mono for machine values). All state coloring is
// done with createShow pairs — an SSR page cannot compute a class, so the
// server picks which statically-styled variant renders.
//
// Server-side data injection contract (see crates/ksx-studio/src/render.rs):
// - every `createSignal("...")` below becomes a NAMED slot in the compiled
//   FMIR (`@getforma/compiler` names the slot after the signal getter), and
//   the Rust side overwrites it by name per request. The defaults here are
//   what renders if injection ever misses — keep them honest ("not
//   collected"), never fake data.
// - each `createList` becomes a uniquely named Array slot (`list:#N:array`,
//   N counting list instances in document order — compiler 0.2.0) and is
//   injected by NAME:
//     list:#1 profile <option>s        list:#2 live virtual-pad tiles
//     list:#3 ghost (empty) pad tiles  list:#4 profile rows with Start
//     list:#5 profile rows, inert
//   Each `createShow` still becomes a shared-name `show:createShow` Bool
//   slot, so shows are resolved by POSITION in the slot table (document
//   order) — the remaining positional seam. The full order lives in
//   render.rs `SHOW_ORDER`; adding/removing/reordering ANY show or list
//   below means updating `SHOW_ORDER` / the `LIST_SLOT_*` constants in the
//   same change. ksx-studio unit tests pin both and fail loudly otherwise.
// - the compiler ignores createShow's condition expression entirely (the
//   server injects the boolean), so the signals referenced there are pure
//   documentation of intent.
// - list item bodies may only use direct member reads (`p.persona`) — in
//   text OR attribute position (compiler 0.2.0 emits a dynamic attr slot
//   for `value: g.title`). Any computed expression makes the compiler fall
//   back to a client island, which this zero-JS page cannot hydrate.
//
// Control contract (docs/CONTROL-SURFACE.md): every button below is a plain
// HTML form POSTing to a route that wraps one backend verb over the daemon
// pipe — /session/start, /session/stop, /config/reload. No JS, no GUI-only
// code paths, CSP untouched. When the daemon is unreachable the same
// controls render disabled, with the reason next to them.

/** Hand-drawn rounded gamepad silhouette (no external assets). Inlined
 *  twice (live + ghost tiles) because the compiler walks h() calls
 *  statically — a shared helper would not be inlined. Keep both copies
 *  identical. */
const SIL_BODY =
  "M20 7 H52 C60 7 63 12 65 19 L68 30 C69.5 36 66 41 61 41 " +
  "C56.5 41 54 37 51.5 33.5 L49.5 31 H22.5 L20.5 33.5 " +
  "C18 37 15.5 41 11 41 C6 41 2.5 36 4 30 L7 19 C9 12 12 7 20 7 Z";

export function StatusPage() {
  const [generatedAt] = createSignal("(no snapshot)");
  const [vigemLine] = createSignal("not collected");
  const [interceptionLine] = createSignal("not collected");
  const [daemonYesNo] = createSignal("unknown");
  const [daemonDetail] = createSignal("not collected");
  const [autostartLine] = createSignal("not collected");
  const [padsSummary] = createSignal("not collected");
  const [profilesSummary] = createSignal("not collected");
  const [configRoot] = createSignal("(unknown)");
  const [sessionLine] = createSignal("not collected");
  const [flashLine] = createSignal("");
  // Booleans injected positionally into the show:createShow slots (see
  // render.rs SHOW_ORDER); the defaults render nothing until the server
  // says otherwise.
  const [pillRunning] = createSignal(false);
  const [pillIdle] = createSignal(false);
  const [pillDown] = createSignal(false);
  const [flashOk] = createSignal(false);
  const [flashError] = createSignal(false);
  const [canStart] = createSignal(false);
  const [canStop] = createSignal(false);
  const [daemonDown] = createSignal(false);
  const [vigemOk] = createSignal(false);
  const [vigemWarn] = createSignal(false);
  const [icptBorrowed] = createSignal(false);
  const [icptAbsent] = createSignal(false);
  const [autostartOn] = createSignal(false);
  const [autostartOff] = createSignal(false);
  const [rowsLive] = createSignal(false);
  const [rowsPlain] = createSignal(false);

  return h(
    "div",
    { class: "studio" },
    // ── App shell: compact header — wordmark + live state pill ──────────
    h(
      "header",
      { class: "top" },
      h(
        "div",
        { class: "brand" },
        h("span", { class: "brand-ksx" }, "ksx"),
        h("span", { class: "brand-studio" }, "Studio"),
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
      // ── SESSION: the hero card ────────────────────────────────────────
      h(
        "section",
        { class: "card hero session" },
        h("h2", null, "Session"),
        h("p", { class: "state" }, () => sessionLine()),
        // Flash = feedback from the LAST action, visually distinct from
        // the state line above (which is the present-tense truth) and
        // always rendered under it. The meta refresh targets "/" so a
        // flash shows once and clears on the next cycle.
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
                  () => [],
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
        { class: "card wide" },
        h("h2", null, "Virtual pads"),
        h("p", { class: "cardline" }, () => padsSummary()),
        h(
          "div",
          { class: "padgrid" },
          createList(
            () => [],
            (p) => p.instance,
            (p) =>
              h(
                "div",
                { class: "padtile live" },
                h(
                  "svg",
                  { class: "sil", viewBox: "0 0 72 48", "aria-hidden": "true" },
                  h("path", { class: "sil-body", d: SIL_BODY }),
                  h("circle", { class: "sil-stick", cx: "22", cy: "18", r: "5" }),
                  h("circle", { class: "sil-stick", cx: "44", cy: "27", r: "4.5" }),
                  h("rect", { class: "sil-dpad", x: "26", y: "25.5", width: "10", height: "3", rx: "1.5" }),
                  h("rect", { class: "sil-dpad", x: "29.5", y: "22", width: "3", height: "10", rx: "1.5" }),
                  h("circle", { class: "sil-dot", cx: "56", cy: "13" , r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "51", cy: "18", r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "61", cy: "18", r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "56", cy: "23", r: "2.2" }),
                ),
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
            () => [],
            (g) => g.slot,
            (g) =>
              h(
                "div",
                { class: "padtile ghost" },
                h(
                  "svg",
                  { class: "sil", viewBox: "0 0 72 48", "aria-hidden": "true" },
                  h("path", { class: "sil-body", d: SIL_BODY }),
                  h("circle", { class: "sil-stick", cx: "22", cy: "18", r: "5" }),
                  h("circle", { class: "sil-stick", cx: "44", cy: "27", r: "4.5" }),
                  h("rect", { class: "sil-dpad", x: "26", y: "25.5", width: "10", height: "3", rx: "1.5" }),
                  h("rect", { class: "sil-dpad", x: "29.5", y: "22", width: "3", height: "10", rx: "1.5" }),
                  h("circle", { class: "sil-dot", cx: "56", cy: "13" , r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "51", cy: "18", r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "61", cy: "18", r: "2.2" }),
                  h("circle", { class: "sil-dot", cx: "56", cy: "23", r: "2.2" }),
                ),
                h(
                  "div",
                  { class: "padmeta" },
                  h("span", { class: "player" }, g.slot),
                  h("span", { class: "persona" }, "empty"),
                ),
                h("div", { class: "instance" }, " "),
              ),
          ),
        ),
      ),
      // ── Card grid: drivers + autostart ────────────────────────────────
      h(
        "div",
        { class: "grid" },
        h(
          "section",
          { class: "card" },
          h("h2", null, "Drivers"),
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
            h("span", { class: "dname" }, "Daemon process"),
            h("span", { class: "dvalue" }, () => daemonYesNo()),
          ),
          h("p", { class: "ddetail" }, () => daemonDetail()),
        ),
        h(
          "section",
          { class: "card" },
          h("h2", null, "Autostart"),
          h(
            "div",
            { class: "drow" },
            h("span", { class: "dname" }, "Logon task"),
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
        ),
      ),
      // ── PROFILES: one row per games.toml entry, one click to start ────
      h(
        "section",
        { class: "card wide" },
        h("h2", null, "Profiles"),
        h("p", { class: "cardline" }, () => profilesSummary()),
        createShow(
          () => rowsLive(),
          () =>
            h(
              "ul",
              { class: "plist" },
              createList(
                () => [],
                (g) => g.title,
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
                () => [],
                (g) => g.title,
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
    ),
    // ── Footer: the plumbing facts, out of the body ───────────────────────
    h(
      "footer",
      null,
      h("p", null, "config root: ", h("span", { class: "mono" }, () => configRoot())),
      h(
        "p",
        null,
        "Status re-read on each request; buttons go over the daemon pipe ",
        "(\\\\.\\pipe\\ksx-daemon). Auto-refreshes every 5 s. Generated ",
        h("span", { class: "mono" }, () => generatedAt()),
        ". Serving 127.0.0.1 only.",
      ),
    ),
  );
}
