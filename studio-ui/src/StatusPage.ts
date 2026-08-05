import { h, createSignal, createList, createShow } from "@getforma/core";

// The cabinet status + session-control page — SSR-only, zero client JS.
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
//     list:#1 profile <option>s, list:#2 virtual pads, list:#3 game profiles
//   Each `createShow` still becomes a shared-name `show:createShow` Bool
//   slot, so shows are resolved by POSITION in the slot table (document
//   order) — the remaining positional seam:
//     shows, in order:  flash, start controls, stop/reload controls,
//                       daemon-down controls
//   Adding/removing/reordering lists shifts the `#N` numbering — update the
//   `LIST_SLOT_*` constants in render.rs; for shows update `SHOW_ORDER`.
//   ksx-studio unit tests pin the list names and the show count and will
//   fail loudly otherwise.
// - the compiler ignores createShow's condition expression entirely (the
//   server injects the boolean), so the signals referenced there are pure
//   documentation of intent.
// - list item bodies may only use direct member reads (`p.persona`) — any
//   computed expression makes the compiler fall back to a client island,
//   which this zero-JS page cannot hydrate.
//
// Control contract (docs/CONTROL-SURFACE.md): every button below is a plain
// HTML form POSTing to a route that wraps one backend verb over the daemon
// pipe — /session/start, /session/stop, /config/reload. No JS, no GUI-only
// code paths, CSP untouched. When the daemon is unreachable the same
// controls render disabled, with the reason next to them.

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
  // Booleans injected positionally into the show:createShow slots; the
  // defaults render nothing until the server says otherwise.
  const [showFlash] = createSignal(false);
  const [canStart] = createSignal(false);
  const [canStop] = createSignal(false);
  const [daemonDown] = createSignal(false);

  return h(
    "div",
    { class: "studio" },
    h(
      "header",
      null,
      h("h1", null, "ksx Studio"),
      h("p", { class: "sub" }, "cabinet status & session control"),
    ),
    h(
      "main",
      null,
      h(
        "section",
        { class: "session" },
        h("h2", null, "Session"),
        createShow(
          () => showFlash(),
          () => h("p", { class: "flash" }, () => flashLine()),
        ),
        h("p", null, () => sessionLine()),
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
              h("button", { type: "submit" }, "Start"),
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
                h("button", { type: "submit" }, "Stop"),
              ),
              h(
                "form",
                { method: "post", action: "/config/reload" },
                h("button", { type: "submit" }, "Reload config"),
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
              h("button", { disabled: "" }, "Start"),
              h(
                "p",
                { class: "warn" },
                "controls disabled — no daemon control channel: ",
                "start the daemon (tray, or `ksx daemon`)",
              ),
            ),
        ),
      ),
      h(
        "section",
        null,
        h("h2", null, "Driver health"),
        h(
          "dl",
          null,
          h("dt", null, "ViGEmBus"),
          h("dd", null, () => vigemLine()),
          h("dt", null, "Interception"),
          h("dd", null, () => interceptionLine()),
        ),
      ),
      h(
        "section",
        null,
        h("h2", null, "Daemon"),
        h(
          "p",
          null,
          "ksx daemon running: ",
          h("strong", { class: "yesno" }, () => daemonYesNo()),
        ),
        h("p", { class: "detail" }, () => daemonDetail()),
      ),
      h(
        "section",
        null,
        h("h2", null, "Virtual pads"),
        h("p", null, () => padsSummary()),
        h(
          "ul",
          { class: "rows" },
          createList(
            () => [],
            (p) => p.instance,
            (p) => h("li", null, h("strong", null, p.persona), " — ", p.instance),
          ),
        ),
      ),
      h(
        "section",
        null,
        h("h2", null, "Autostart"),
        h("p", null, () => autostartLine()),
      ),
      h(
        "section",
        null,
        h("h2", null, "Game profiles"),
        h("p", null, () => profilesSummary()),
        h(
          "ul",
          { class: "rows" },
          createList(
            () => [],
            (g) => g.title,
            (g) => h("li", null, h("strong", null, g.title), " — ", g.detail),
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
        "Status re-read on each request; session state and buttons go over ",
        "the daemon pipe (\\\\.\\pipe\\ksx-daemon). Auto-refreshes every 5 s. ",
        "Generated ",
        () => generatedAt(),
        ".",
      ),
      h("p", null, "config root: ", () => configRoot()),
      h("p", null, "Serving 127.0.0.1 only."),
    ),
  );
}
