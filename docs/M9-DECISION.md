# M9 — the native UI: decision (2026-08-06)

**Decision: M9 as specified (an egui/eframe config UI) is CANCELLED, not
deferred. M9 becomes "ksx is a real Windows application" — an owned icon, a
Start Menu entry, a tray "Open ksx" item, and a launcher verb that starts the
daemon and opens Studio in a chrome-less application window — and ksx Studio is
the UI. The reason is not that egui is bad; it is that E7's own justification
for a native UI has already been satisfied by something else.** E7 asked for the
native UI so the GUI could host the supervisor in-process "with no serialization
tax, mapping 1:1 to `DaemonCommand`". `daemon/tray.rs` had already rejected
in-process for UI on purpose ("it owns no channel to the capture thread, the
engine thread or the output thread… its only outbound edge is a
`Sender<DaemonCommand>`"), and CONTROL-SURFACE then granted the pipe **exactly
the tray's reach**. The reach is identical; the "tax" is one JSON line per human
click, at start/stop/reload/map rates of a few per minute, against an
ARCHITECTURE rule-5 budget (p99 < 1 ms) that lives in the capture thread neither
UI can touch. The performance argument for egui is empirically void. What E7 was
actually protecting is the process boundary — and the pipe protects it better
than in-process would.

---

## 1. What the plan assumed, and why the assumption is gone

E7 was written **2026-08-04**, in the same 24 hours the Studio skeleton was
committed (`d4d96ea`, 2026-08-04 22:09). At the moment "Native primary
(non-negotiable) — CLI + tray daemon (M5) + native config UI (M9). Zero HTTP,
zero web deps in the default build" was typed, **ksx Studio was an empty page
that rendered a status table**. The native UI was the only plan for a mapper,
so making it non-negotiable cost nothing.

Then Studio was built. Measured today:

| | |
|---|---|
| Wall clock | 25 commits, **2026-08-04 22:09 → 2026-08-06 12:58 (~39 hours)** |
| Production lines | **23,257** — `ksx-studio/src` 8,929 (`render_map.rs` alone 5,809), `ksx-app/src/studio.rs` 1,310, `studio-ui/src` 12,773 (`MapIsland.ts` 5,599, `studio.css` 3,853, `map.ts` 2,155), `build.mjs` 245 |
| Tests | **139** — 102 Rust `#[test]` in `ksx-studio` + 7 in `ksx-app/src/studio.rs` + 30 browser tests in `studio-ui/pwtest` (+1,249 lines of `tests/http.rs`) |
| Surface | 27 routes, every one 1:1 onto one of the pipe's 12 verbs through one writer |
| Features with no egui equivalent | 37-column diagonal piano roll, vendored controller art with measured hit-test extents, first-class diagonals across three mechanisms, multi-select + multi-bind, toasts + undo, no-JS form twins, a documented design system (DESIGN-SYSTEM.md) |

**This is a legitimate re-decision, not a betrayal of the plan.** The plan's
sequencing rule — "nothing web-related precedes M6" — was about the *driver
deadline outranking the showcase*, and it was honoured: M6.5, M7 and M8 all
landed before Studio grew past a status page. What was never decided, because
the question could not be asked in advance, is *which surface should own the
mapper once one of them actually had a mapper*. A plan that says "build the
native mapper" after the other mapper exists and is better is not discipline,
it is sunk-cost with a footnote. E7's constraint stands unchanged in substance;
one clause of it is amended in §8.

The clause that must be amended honestly rather than reinterpreted: **"zero web
deps in the default build"** and **"the cabinet works perfectly with no browser
in existence."** Under this decision the default build still links no axum, no
forma, no tokio — `--features studio` still gates all of it, still provable with
`cargo tree`. But the *GUI* now depends on a Microsoft-shipped web engine being
present on the machine. That is a real reduction in the promise and §5 states
its exact cost rather than talking around it.

---

## 2. The three options, honestly priced

Two columns because this repo's demonstrated velocity and a conventional
estimate differ by roughly 4×, and quoting only one of them is how these
documents lie. The conventional column is "one competent Rust dev who already
knows the toolkit"; the observed column is calibrated against the 39-hour /
23k-line Studio build above. **The last column is the one that decides it.**

| Option | Conventional | At this repo's velocity | Cost per future feature |
|---|---|---|---|
| **A — egui/eframe native UI** | **44–57 d (≈50)** | **11–17 d** (6–10 UI + 5–7 test harness, which does *not* compress) | **2× forever** |
| **B — native shell hosting Studio's UI in WebView2** | **15–20 d** (of which 3–4 d is the `ksx-api` refactor wanted anyway) | **8–12 d** | 1× |
| **C — minimal native identity, Studio is the UI** | **3–6 d** | **2–3 d** | 1× |
| **`ksx-api` (§6)** — charged once, under *every* option | 3–4 d | 2–3 d | **reduces** it |

### Option A — egui/eframe, as M9 was specified

egui *can* express nearly all of it, and the brief's "months of work" framing is
wrong: the mapper geometry is already Rust (`ZONE_XBOX: [Zone; 25]` /
`ZONE_DS4` in `render_map.rs`, stage-percent `[cx, cy, w, h]` boxes with labels
and palette keys, directly consumable as egui `Rect` math), the art is a static
silhouette that needs one rasterization per (persona, theme, size) because
`studio-ui/art/README.md` records that its paths are *not* individually
addressable, and a 37×N grid of toggles is immediate mode's home turf — better
than the CSS version, with no `overflow-x` gymnastics.

Three costs are small and named: bundling a subset font for `✕ ○ △ □` and
`↑ ↓ ← → ↖ ↗ ↙ ↘` (egui's bundled fonts do not reliably cover them → tofu; ~½ d);
`@media (prefers-color-scheme)` inside `pad-xbox.svg` being outside resvg's
tested scope, so the free theme switch becomes string substitution or two baked
variants (~30 lines + a golden test); and re-rasterization on resize, plus the
known egui SVG-loader panic on a zero-width size hint.

Two costs are not small:

- **The design system lands at ~85% and still reads as an egui app.**
  DESIGN-SYSTEM.md specifies a 9-step type scale, 10-step spacing, a 3-step
  control-height ladder with a `pointer: coarse` branch, a 6-step radius scale
  with "a control's radius is never larger than its container's", a 4-step
  elevation ladder, ~12 named colour roles and ~25 component classes. egui's
  `Style`/`Visuals` offers one coarse `Spacing` struct, rounding per
  *interaction state* rather than per component, shadows only on
  `Window`/`Popup`/`Tooltip`, and no focus-ring primitive with offset.
  Everything past the defaults is `Painter` calls in custom widgets — 600–900
  lines of theme code to arrive somewhere short of where the CSS already is.
- **egui cannot serve the phone at all.** E7's stated reason for Studio is
  "configure the cabinet from your phone while standing at it — the case where a
  browser is genuinely the right client, since a cab has no keyboard." An egui
  window *on the cabinet screen* needs a keyboard and mouse at the cabinet,
  which is the problem Studio exists to solve. **Option A is not a substitute
  for Studio on the one use case Studio was specified for**, which means A does
  not replace Studio — it is a second UI, permanently.

That last point is what converts A's cost from a one-time number into a
multiplier. The open work is large: `ksx slot assign`, per-slot persona, the
claimed-panel learner, MAPPER-UX Builds B and C, four explicitly-deferred items,
plus every future CONTROL-SURFACE verb. Under A each one is designed, built,
tested and documented twice, in two idioms, forever. **That is the argument
against A. It is not the build cost.**

### Option B — a native shell hosting the existing UI (WebView2)

Verified rather than assumed, on this machine, today: the WebView2 Evergreen
Runtime is present, per-machine, **151.0.4129.59**. Windows 11 ships it in-box;
Windows 10's tail is covered by the 2 MB Evergreen bootstrapper, gated on the
same registry key `ksx doctor` should learn to read.

The zero-HTTP claim is real and mechanical: WebView2's
`AddWebResourceRequestedFilter` + `WebResourceRequested` +
`CreateWebResourceResponse` serve `ksx://…` requests **in-process, with no
listener, no socket and no port** — provable at runtime by `netstat` showing
nothing. `forma_server::render_page(&PageConfig) -> PageOutput` is already a
synchronous pure function with no tokio dependency, so HTML generation never
needed HTTP; only `server.rs` did.

The correct implementation is **`webview2-com` directly, not wry and emphatically
not Tauri**: wry pins `windows ^0.61` against our workspace's `windows = "0.62"`
(resolved 0.62.2) and would fork the `windows` crate in our tree while shipping
an event-loop abstraction we deliberately do not have — `tray.rs` already
hand-rolls `RegisterClassW` + `CreateWindowExW` + `GetMessageW` with the
rationale written in its doc comment. The shell window is the same five Win32
calls. Tauri adds a CLI, a config file, a capability ACL, an updater and a
bundler, and assumes a static SPA, while our page is server-rendered per request.

B is a genuinely good design. **It is not cancelled — it is priced, sequenced
behind C, and gated on named triggers (§7).**

### Option C — minimal native identity; Studio is the UI

`msedge.exe --app=http://127.0.0.1:PORT/ --user-data-dir=<ksx profile>` gives a
frameless window with no address bar and no tabs, its own taskbar button, its own
alt-tab entry, and no inherited extensions or session. What remains missing is
identity work ksx needs *under every option*: today the tray calls
`LoadIconW(ptr::null_mut(), IDI_APPLICATION)` — **the generic Windows default
icon** — there is no Start Menu entry, no Add/Remove Programs presence, no owned
`.ico` outside `legacy/`, and `ksx studio` does not open a browser at all; the
user is expected to type a URL.

That last one is the whole decision in miniature. The thing that makes ksx feel
like a web page is not HTML — it is that you launch it by typing a URL, and that
clicking a shortcut before the daemon is up would hand you
`ERR_CONNECTION_REFUSED`. **Both are launcher bugs, and both are fixed in a day.**

---

## 3. Why C first, and why B is the upgrade rather than the alternative

B decomposes cleanly into two halves, and only one of them is about WebView2:

1. **Lift the routes out of axum into one transport-agnostic dispatch.** This is
   B's load-bearing item and B's largest risk if skipped ("skip it and you fork
   Studio"). It is *also* `ksx-api` (§6), which is worth building under A, B and
   C alike.
2. **Host a WebView2 control in our own HWND and serve the assets through the
   custom-protocol handler.** 12–16 conventional days of COM plumbing, lifecycle,
   settings lockdown, deferrals and DPI, whose entire user-visible yield over C
   is: no right-click Reload/Print/Inspect, no live Ctrl+P / Ctrl+S / F12, no
   per-origin zoom that a stray Ctrl+scroll can leave at 150% forever, and no
   dependency on a *browser* (only on the runtime).

Do (1) now because we want it anyway. Do (2) when one of §7's triggers fires.
Both C and B depend on a Microsoft web engine; **B does not buy back the
no-web-engine promise**, so it is not the answer to §5's loss and must not be
sold as one.

Cabinet-specific losses under C are smaller than they look. `learn-key` is
already refused while a session runs (four documented reasons in
CONTROL-SURFACE), and MAPPER-UX commandment 9 makes mapping a between-games
activity — you are already stopped and already alt-tabbed, so "a window pops over
Big Box and drops exclusive fullscreen" describes a state you are not in. The
genuine kiosk loss is a shell-replacement setup (Big Box as the shell, no
`explorer.exe`) — but there **the tray does not exist either**, so native and web
lose identically and the CLI is the only surface. That is a wash, not a
differentiator.

---

## 4. What ships as M9

Scope, in order. Roughly 700–1,000 lines and one `.ico`.

1. **`ksx open` (and tray → "Open ksx")** — the launcher, and the single most
   important item here. It must: check whether the daemon is running; start it
   if not; poll the port until the page answers; *then* open the window. It must
   never be possible to reach `ERR_CONNECTION_REFUSED` by clicking a ksx
   shortcut. If it cannot open a window it says so and stops, per
   CONTROL-SURFACE's "a surface that cannot act must SAY so" invariant.
2. **Launch by App Paths, never `ShellExecute` on a URL** — resolve
   `HKLM\…\App Paths\msedge.exe` (verified present on this machine) and exec it
   with `--app=` + a ksx-owned `--user-data-dir`. This survives a stripped image
   with no default `http` association, kills extension injection, inherits none
   of the user's tabs, and keeps zoom state out of the user's browser profile.
   Fall back to the default browser only if no Chromium binary is found, and say
   which one it used.
3. **An owned icon**, replacing `IDI_APPLICATION` in `tray.rs`, and used as the
   window icon, the shortcut icon and the favicon Chromium derives the taskbar
   icon from.
4. **Start Menu entry + Add/Remove Programs presence** in the installer.
5. **Single instance** — a second `ksx open` focuses the existing window.
6. **`ksx doctor` rows** for browser presence and (in advance of B) WebView2
   runtime presence, at Info severity, with the exact remedy line.
7. **Keep `ksx studio` as a separate process that reads config from disk** and
   renders the mapper read-only behind the "No daemon" banner. This is the
   recovery path when the daemon is wedged; folding it into the daemon would
   delete it.

Non-goals for M9: any WebView2 work, any second rendering of any screen, any
change to the pipe protocol.

---

## 5. What we lose by not building egui — stated without flinching

1. **E7's literal promise dies.** "The cabinet works perfectly with no browser
   in existence" is no longer true of the *GUI*. On a machine with no Chromium
   and no WebView2 — Windows LTSC/IoT with Edge stripped, a hardened image, a
   future Windows that unbundles it — ksx has **no graphical mapper at all**.
   That is a genuine capability we are choosing not to have.
   **What survives on such a machine, and it is not nothing:** the CLI is a
   *complete* surface by contract — CONTROL-SURFACE's standing rule is that
   every front-door action maps to an existing backend verb, so there is no
   operation reachable only from a GUI — plus the tray for start/stop/reload/
   status, which is native Win32 and needs no web engine ever. The bounded loss
   is the *graphical* mapper, not ksx.
2. **A dependency we do not control is now on the GUI's critical path.** Edge
   updates itself; a Chromium regression, a policy change to `--app=`, or an
   enterprise lockdown is now our outage. egui would have had exactly one
   external dependency: a GPU that can draw triangles.
3. **Startup and footprint.** An egui window is a few MB of process and paints in
   tens of milliseconds. A Chromium window is hundreds of MB — 6 `msedgewebview2.exe`
   processes were already resident at 250 MB on this machine — and takes a second
   or two to first paint. On a cabinet PC that is affordable; it is not free, and
   pretending otherwise would be dishonest.
4. **Offline-by-construction as an auditable property.** "This binary cannot
   speak HTTP" is a stronger, cheaper security story than "this binary speaks
   HTTP to itself on loopback, gated behind a feature flag." The feature gate
   keeps the *default build* clean, so the property survives for the shipping
   default — but the GUI build no longer has it.
5. **The purity of the premise.** "The input path must be boring and auditable"
   is this project's whole character, and there is a real aesthetic cost to the
   config surface being a browser. This document does not argue that cost away.
   It argues that 23,257 lines of finished, tested, documented UI outweigh it,
   and that the CLI keeps the premise intact where it actually matters.

---

## 6. The one thing that is worth building under every option: `ksx-api` (M10a)

Under A, B and C alike, the highest-value work is the same: **one typed,
in-process, transport-free API that every surface consumes.** It is what makes A
survivable if we ever build it, what makes B a two-week job instead of a rewrite,
and what makes C's Studio-only world safe to live in.

**It is also already 80% written, in the wrong crate.** `ksx-studio` contains
`StatusSource` (`snapshot.rs`) and `ControlSource` (`control.rs`) with their view
types, and `ksx-app/src/studio.rs` contains the two real implementations
(`CollectorSource` over the config store and platform collectors,
`PipeControlSource` over `\\.\pipe\ksx-daemon`). M10a is mostly a **move plus two
additions**, not a design exercise.

### Shape

**New crate `crates/ksx-api`.** Dependencies: `serde`, `serde_json`,
`thiserror`, and the ksx crates it names types from. **No axum, no forma, no
tokio, no HTTP types, no `async`, not even behind a feature.** If `ksx-api`ever
grows a dependency that can open a socket, this decision has been undone by
accident.

**Two traits, kept separate — the split is load-bearing:**

```rust
/// Read side. Satisfiable with NO daemon running: ksx-app's collectors read
/// the config store and the platform directly. This is why `ksx studio`
/// renders a read-only mapper behind the "No daemon" banner instead of an
/// error page, and merging it into the write trait would delete that.
pub trait StatusSource: Send + Sync {
    fn snapshot(&self) -> StatusSnapshot;
    fn mapper(&self) -> MapperSnapshot { MapperSnapshot::unavailable(…) }
    fn macros(&self, preset: &str) -> MacroSnapshot { MacroSnapshot::unavailable(…) }
}

/// Write side + live session state. Needs a reachable daemon; every method is
/// one DaemonCommand or one CLI verb's implementation, and NOTHING else — the
/// tray's reach exactly (CONTROL-SURFACE invariant 1).
pub trait ControlSource: Send + Sync {
    fn session(&self) -> SessionView;
    fn start(&self, profile: Option<&str>) -> Result<String, Refusal>;
    fn stop(&self)   -> Result<String, Refusal>;
    fn reload(&self) -> Result<String, Refusal>;

    fn learn_start(&self)  -> LearnView;
    fn learn_poll(&self)   -> LearnView;
    fn learn_cancel(&self) -> LearnView;

    /// The whole key list for one control, in ONE atomic write.
    fn bind_keys(&self, preset: &str, function: &str, keys: &[String],
                 force: bool, reload: bool, turbo_hz: Option<u32>) -> BindOutcome;
    fn restore(&self, preset: &str, mode: &str) -> Result<String, Refusal>;
    fn clear_all(&self, preset: &str)           -> Result<String, Refusal>;
    fn save_macro(&self, request: &MacroWrite)  -> MacroOutcome;
}
```

Every defaulted method keeps its current behaviour: an honest, *worded*
"unavailable" rather than a silent no-op — that default is what makes a partial
implementation say which provider call is missing instead of rendering an empty
grid that looks like real data.

**One refusal type, replacing the bare `String` errors:**

```rust
pub struct Refusal {
    /// The pipe's stable code — `conflict`, `unknown-preset`, `macro-invalid`,
    /// `no-channel`, `not-running`, `already-running`, … Same words the CLI's
    /// --json prints and the exit codes derive from. Never invented per caller.
    pub code: &'static str,
    /// One sentence naming what was refused and why.
    pub message: String,
    /// The command that works anyway, when one exists — the `ksx map` one-liner
    /// the "No daemon" banner already prints. This field is how
    /// CONTROL-SURFACE's "a surface that cannot act must SAY so, per click"
    /// invariant becomes a type obligation instead of a review checklist item.
    pub remedy: Option<String>,
}
```

**Three implementations, all satisfying the same traits:**

| impl | how it reaches the daemon | consumer |
|---|---|---|
| `PipeClient` | one `\\.\pipe\ksx-daemon` JSON line per call — today's `PipeControlSource`, moved | Studio, `ksx session`, the E5 MCP shim, a shell in a *separate* process |
| `InProcess` | holds `Sender<DaemonCommand>` + `SharedState` + the `mapping::*` writers; no serialization | a UI hosted **inside** the daemon process — this is E7's "no serialization tax", delivered as an implementation of the shared trait rather than as a second UI |
| `Collectors` (read side) | config store + platform collectors, no daemon required | the dead-daemon read-only path |

**The property that makes this the right investment:** a surface is written once
against the traits and can then be hosted either way. If we ever build the egui
UI, it starts against `InProcess` and gets E7's in-process ideal *for free*, with
no second copy of any verb. If we build B's WebView2 shell, its custom-protocol
handler is a second thin adapter over the same dispatch. If we build neither,
Studio and the MCP server share one contract and one test suite.

**Adapters, not logic.** `ksx-studio`'s axum layer becomes ~27 route functions
that unwrap a request, call one trait method and render the answer — which is
very nearly what `server.rs` already is ("everything below is the SAME
`ControlSource` verb the `/api/*` route above it uses"). No route may contain a
decision that a non-HTTP caller would need to re-implement. That rule is the
whole point; violating it is how Studio gets forked.

**Testing.** One fake implementation of each trait drives every surface's tests —
`ksx-studio/tests/http.rs`'s fixtures already work this way, so the 109 existing
Rust tests survive the move.

**Cost: 2–4 days.** Charged once. It is the only item in this document that
*reduces* the cost of every future feature under every option, which is why it
should be built before Mapper Build B, before `ksx slot assign`, and before any
WebView2 work.

---

## 7. If this decision is wrong: what is reversible, and what is not

**Reversible, cheaply — everything in §4.** The launcher, the icon, the Start
Menu entry and the tray item are the same work under all three options. None of
it is thrown away by later building B *or* A; a native shell wants an icon and a
Start Menu entry just as much. **There is no lock-in in shipping M9 as C.**

**Reversible, at its stated price — Option B.** The 12–16 conventional days of
WebView2 host are payable at any later date and become *cheaper* once `ksx-api`
exists, because the custom-protocol handler is then one more adapter. Nothing in
C forecloses B; C's launcher verb is the thing B replaces, and it is a day's work.

**Reversible, expensively — Option A.** egui remains buildable forever, on top of
`ksx-api`'s `InProcess` implementation, at 11–17 observed days for parity with
Studio *as it stands today* — a number that grows with every feature Studio gains
in the meantime. This is the real irreversibility: not that we could not build
it, but that the parity target keeps moving.

**Not reversible.** Nothing. There is no data migration, no schema change, no
protocol change and no on-disk format change in this decision. Config remains
hand-editable TOML, edits still land by re-reading those files, and every write
still goes through one writer.

### Triggers that should flip the decision (write these down so they are checked, not felt)

- **Flip C → B** if any of: a cabinet user hits the Chromium context menu /
  Ctrl+P / F12 / stuck-zoom failure mode in real use; Edge's `--app=` behaviour
  changes or is disabled by policy on a target machine; the launcher cannot
  reliably produce a clean window in under ~2 s; or a target machine has the
  WebView2 runtime but no usable Edge *browser*.
- **Flip → A (egui)** only if: ksx must run a GUI on a machine with no web
  engine at all *and* the CLI plus tray provably do not cover that user — i.e.
  someone actually needs graphical mapping on an LTSC/IoT cabinet. That is a
  real scenario; it has just never been observed here.
- **Do not flip on aesthetics alone.** "It would feel more native" is what this
  document has already weighed, at ~50 conventional engineer-days and a permanent
  2× on every future feature.

---

## 8. Amendment to E7

E7's Sequencing paragraph is updated in `docs/ENHANCEMENTS.md` to match this
decision. The substance of E7 is unchanged: the default build still links no
axum, no forma and no tokio (`--features studio`, provable with `cargo tree`);
Studio still never touches a pipeline thread; the monitor still coalesces to
display rate; localhost is still the default and LAN is still an explicit opt-in
with a CSPRNG pairing token. The three-surfaces list keeps surfaces 2 and 3 as
written. Surface 1's parenthetical "native config UI (M9)" is what changed, and
the "zero web deps in the default build" clause now carries the honest scope
noted in §1 and §5: it is a statement about the *default build*, not a promise
that a GUI exists on a machine with no web engine. On such a machine the CLI —
complete by CONTROL-SURFACE's standing rule — and the tray are the surfaces.
