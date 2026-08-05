import { h, createSignal, createList } from "@getforma/core";

// The cabinet status page — SSR-only in the M10 skeleton.
//
// Server-side data injection contract (see crates/ksx-studio/src/render.rs):
// - every `createSignal("...")` below becomes a NAMED slot in the compiled
//   FMIR (`@getforma/compiler` names the slot after the signal getter), and
//   the Rust side overwrites it by name per request. The defaults here are
//   what renders if injection ever misses — keep them honest ("not
//   collected"), never fake data.
// - each `createList` becomes a `list:array` slot. The compiler gives EVERY
//   list the same slot name, so the Rust side resolves them by POSITION in
//   the slot table, which follows document order. There are exactly two
//   lists on this page, in this order: virtual pads, then game profiles.
//   If you add/remove/reorder a list, update `LIST_ORDER` in render.rs —
//   a ksx-studio unit test pins the count and will fail loudly otherwise.
// - list item bodies may only use direct member reads (`p.persona`) — any
//   computed expression makes the compiler fall back to a client island,
//   which this zero-JS skeleton cannot hydrate.

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

  return h(
    "div",
    { class: "studio" },
    h(
      "header",
      null,
      h("h1", null, "ksx Studio"),
      h("p", { class: "sub" }, "cabinet status"),
    ),
    h(
      "main",
      null,
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
        "Point-in-time snapshot re-read on each request (no daemon IPC yet) — ",
        "auto-refreshes every 2 s. Generated ",
        () => generatedAt(),
        ".",
      ),
      h("p", null, "config root: ", () => configRoot()),
      h("p", null, "Serving 127.0.0.1 only."),
    ),
  );
}
