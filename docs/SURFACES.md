# Surfaces: what each one is for

ksx has three faces — the CLI, the egui cabinet app, and Studio in a browser —
and one backend. This file records which face owns what, in what order they get
built, and the alternatives that were considered and rejected. It exists so that
"which surface does this go on?" is answered once instead of re-argued per
feature.

Written 2026-08-07, after `ksx device pick` shipped as a verb with no web face
and the question came up for the fourth time. Audited and corrected the same
week — the matrix had four cells describing capabilities that do not exist, and
§1's supporting anecdote was false in both halves. Every correction below names
the code it was checked against.

> This file is **cited from source**, like every other design doc here. That is
> not decoration: at its first audit `SURFACES.md` had zero references anywhere
> in the repository — `.rs`, `.md`, `.toml`, `.ts` — while `INPUT-TRANSFORMS.md`
> had 106 and `CONTROL-SURFACE.md` 29. A design document nothing points at is a
> memo, and a memo cannot be violated because nobody is looking at it when they
> write the code. `crates/ksx-app/tests/docs.rs` now fails if a governing doc
> loses its last citation.

## §1 The backend owns state; every surface is a view

Already the architecture, stated here so it stops being implicit. From
`crates/ksx-app/src/device_edit.rs`:

> One writer, like every other write. A typed spec in, a pure plan out, a
> timestamped backup taken before the write, and the store's atomic save doing
> the I/O.

And from `ksx studio --help`:

> every button is one backend verb, no GUI-only code paths.

**No surface may hold logic another surface would need.** A capability becomes a
typed spec and a pure plan in the backend; surfaces call it and render the
result. `ksx-api` is the wire contract between them.

### Where this is currently broken, on purpose and by accident

Stated here because an unqualified rule that the code visibly does not follow
teaches people to ignore the rule rather than to fix the code.

- **The mapper's timing arithmetic exists three times.** `ksx_core` owns it;
  `ksx-studio/src/render_map.rs` mirrors `MIN_STEP_MS`, `TURBO_MAX_HZ`, the
  frame maths, `MacroStep::effective_ms`, the turbo on/off split, the turbo gap
  and two SOCD/diagonal helpers, each with a comment saying it is a mirror; and
  `studio-ui/src/MapIsland.ts` mirrors the same values a third time so the
  no-JS page and the interactive island agree. The Rust copy is pinned to
  `ksx_core` by a test. **The TypeScript copy is pinned to nothing**, which
  makes it the one that will drift, and it will drift silently because a wrong
  step preview looks like a wrong step preview and nothing else.
- **The mappable-function vocabulary lives in the surface.** `ZONE_XBOX` and
  `ZONE_DS4` in `render_map.rs` hold the 25 canonical pad functions, and the
  test that checks them re-types the same 25 strings, so adding a function to
  `ksx-core` fails nothing. The bindable-*key* vocabulary next to it does this
  right — it is pinned against `ksx_core::key::Key::ALL` through a test-only
  dev-dependency — and the fix is the same shape: a canonical `ALL` on the
  function type, then compare against it. There is no such list to compare
  against yet, which is why this is still written down instead of tested.
- **The egui takes a few decisions of its own.** `Screen::ButtonCheck`'s action
  clears a local log, `Screen::Status`'s prints a nag, and `Ask::Refresh` is a
  re-read: three actions that are not backend verbs, in a file whose own comment
  says there is no such variant. All three are state-free, so the harm is nil
  and the claim is what is wrong, not the code.

The cost of breaking this rule is not theoretical — but the anecdote that used
to sit here was, and it is corrected rather than deleted because the true
version argues the same point from the opposite direction.

**What was written:** `MachineSource::devices()` sat with no implementation
while the cabinet UI had a devices screen, so the screen could not list a board.

**What actually happened:** the cabinet has never had a devices screen —
`ksx-cabinet/src/nav.rs` has always been exactly ButtonCheck, Status, Session,
Profiles, Presets, and `git log -S` finds no deleted one. `devices()` *is*
implemented now (`ksx-app/src/sources.rs`), and has **zero callers**: the only
`.devices()` calls in the tree are capture backends and the trait's own test.
The backend read was built for a screen nobody wrote, and is compiled behind
`#[cfg(all(windows, feature = "cabinet"))]` for a consumer that never asks.

So the failure mode is symmetric, and the rule needs both halves: a surface must
not own a capability the backend lacks, **and a backend verb with no face is not
finished either** — it is untested against a real caller, and its shape is a
guess. That is the surface-parity guard on the task list (#26), and until it
exists this paragraph is the only thing checking.

## §2 Build order

1. **Backend verb** — typed spec, pure plan, tested against synthetic fixtures.
2. **CLI** — the cheapest surface to test and the one CI can drive headlessly.
3. **The surface the task is actually performed on** (§3, §4).

There is no "egui first or web first" question. That framing assumes a surface
owns a capability, which §1 forbids. The real question is only ever *which
surface does a human perform this task on*, and that is answered by the matrix.

## §3 The capability matrix

| Capability | CLI | egui (cabinet) | Studio (browser) |
|---|---|---|---|
| Author presets / key mappings | owns | — | **primary** |
| Edit config, profiles | owns | slot→preset only | planned primary |
| Device pick / remove | owns | planned (#22) | planned (#22) |
| WinUSB claim / release | owns | planned | never (needs elevation) |
| "Press a button, see it light" | input only (`ksx monitor`) | **primary** | planned (§8) |
| Is it working: pads, drivers | owns | **primary** | view |
| Start / stop / switch profile | owns | **primary** | convenience |

"owns" = the verb lives here. "primary" = where a human does it. "view" =
renders backend state, takes no decisions. **"planned" = nothing is there** —
and it is spelled out because the previous version of this table used "view" for
two cells that render nothing at all, which reads as a shipped capability and
cost an audit to catch.

Four cells were corrected, each against the code:

- **Edit config — egui.** Was "—". The egui *does* write config: the Presets
  screen builds an `Ask::Assign`, which becomes a `SlotAssignRequest` that
  rewrites a `[[slot]]`'s preset. It also decides **which file** the write lands
  in (`config.toml` or a `games.toml` profile) in `assign_destination`, whose
  own doc comment records the data-loss bug the previous version of that
  decision caused. That is a targeting decision taken in a surface; it is the
  strongest live counter-example to §1 in the tree, and it stays until the
  destination rule moves behind the verb.
- **Edit config — Studio.** Was "**primary**". No Studio route writes
  `config.toml` or `games.toml`; the only config-adjacent route re-reads. Every
  `/map/*` write goes to a preset file. `AppState` holds no `MachineSource`, and
  `ControlSource` has exactly one config-writing verb (`slot-assign`) which
  Studio never calls. It is the right destination, so it stays in the table —
  as a plan.
- **Device pick / remove, WinUSB claim / release — egui.** Both were "view".
  The cabinet has five screens and none of them is devices; the only device
  string anywhere on that surface is a truncated board name inside the
  button-check log. `MachineSource::winusb()` is a defaulted refusal with no
  implementation and no caller.
- **"Press a button, see it light" — CLI and Studio.** The CLI cell was "—",
  but `ksx monitor` streams `<alias> <Key> down|up` per keystroke: that is the
  input half, and what it lacks is the second column the egui has (what the
  panel sent *and* what the pad published — screens.rs explains that the pair is
  the diagnostic). The Studio cell claimed "useful on phone (§6)": there is no
  live-input channel in Studio at any screen size — no feed on `AppState`, no
  frame type, nothing — and the cross-reference pointed at the wrong section
  (mobile is §8; §6 is launching).

## §4 The egui is an appliance panel, not a worse browser

The egui's five screens (`ksx-cabinet/src/nav.rs`) are ButtonCheck, Status,
Session, Profiles, Presets — and ButtonCheck is described in the source as "the
spine". That is correct and it generalises:

**At an arcade cabinet there is no mouse and no keyboard. The panel is the
input.** A browser UI cannot be driven by an arcade stick; the egui already
responds to panel presses. This is a structural advantage no web surface can
take, and it is why "just use Studio for everything" is not on the table.

The egui's job is therefore everything a person does **standing at the machine,
with only the panel to touch**: confirm it works, start and stop, switch
profile. Anything requiring text entry belongs elsewhere.

## §5 Studio is the workbench

Studio binds `127.0.0.1` and refuses anything else — `ksx-studio/src/error.rs`
returns `NonLoopbackBind` rather than serving a LAN address. It currently has
two **pages**, `/` and `/map`.

Two pages, twenty-seven routes: the rest are `/api/status`, fourteen mutating
`/api/*` and `/map/*` endpoints, the service worker, the asset handler and three
icons. The distinction is not pedantry — it is the whole reason the CSRF guard
is one layer over the router rather than a check per handler, because "the
mapper alone grew eight form endpoints in three milestones" and the failure mode
being prevented is forgetting one.

`/map` is good enough to stop treating as supplemental tooling. Authoring a
25-binding preset is a pointer-and-keyboard task and the browser is simply
better at it than immediate-mode GUI. **Studio is promoted to a core surface for
authoring** — co-equal with the egui, not above it.

It is deliberately *not* promoted to primary for operating. If Studio were the
only way to start a session, a cabinet in attract mode would need a web server,
a free port and a browser running before anyone could play. That is a worse
appliance than the one that exists now.

## §6 Launching goes egui → Studio, never the reverse

The cabinet app has an "Open Studio" action. That direction is correct: the
process already running at the machine can open the workbench.

**The reverse was rejected.** A browser cannot launch a native app without a
registered custom protocol (`ksx://`), which means writing registry entries at
install time and a security prompt on every click — real friction and a real
install-time footprint, bought for a convenience the egui already provides in
the direction that works.

## §7 LAN access needs a pairing token, and the guard needs to learn about it

`ksx studio --help` already records the intent:

> Localhost only — there is no LAN option; that arrives with the pairing token.

**A LAN bind is not the same class of problem as a Node dev server on a laptop.**
That comparison is tempting and wrong. A dev server serves files nobody attacks;
Studio can **start and stop input capture, claim and release USB devices, and
rewrite config**. A home network is not a trust boundary — a guest phone, a smart
TV or a compromised IoT device is on the same WiFi.

So LAN access is: bind beyond loopback, require a token, reject anything
unauthenticated. Two consequences worth writing down before the work starts:

- **Discovery is a QR code in the egui.** Nobody types
  `http://192.168.1.47:4460/?t=xK9m…` on a phone. A copyable URL is the laptop
  fallback, not the primary path.
- **`ksx-studio/src/guard.rs` will reject it — twice, and the second one is the
  easy miss.** There are two independent checks and a LAN bind has to clear
  both:

  1. `is_loopback_host` on the `Host` header, the DNS-rebinding defence. A LAN
     address fails it, and the request never reaches a handler (421).
  2. `is_own_origin` on the `Origin` header of every mutating request. A form
     posted from `http://192.168.1.47:4460` fails it too, because the origin
     check ends in *and the host is a loopback name*.

  Fix only the first and you ship a Studio that **renders on the phone and
  refuses every button** — a 403 on every form, which reads as a broken app, not
  as a security feature. Both checks now consult the bound address, so each
  passes for the address ksx is actually serving on; that half is inert while
  the bind stays loopback-only, and it is pinned by two tests in `guard.rs` so
  the LAN change-set cannot land with the trap still in it. What is still
  missing is the token, which is the part that makes the bind *safe* rather than
  merely *working*.

  Writing those tests turned up the same shape of bug already shipped: the two
  checks disagreed about `[::1]`. `Host` accepted it, `Origin` did not — an
  already-split IPv6 name was being re-split on its own colons, so `::1` became
  `::` — and a dual-stack machine that landed on the IPv6 loopback got a Studio
  that rendered and 403'd every form. Exactly the failure this bullet predicts,
  arriving early and by a different route.

## §8 Mobile: responsive-only, aimed at diagnostics

No dedicated touch layout yet, and no deferring mobile either — because there is
one phone use case that beats every other surface:

**You are behind the cabinet with the panel open, pressing buttons, and the
phone in your hand shows which key fired.** That is ButtonCheck on a phone, and
it is better than walking round to the monitor for every wire.

Two corrections to how that reads, both of which change what the work *is*:

**"Responsive-only" currently means "not responsive at all."** There is no
`<meta name="viewport">` anywhere in `studio-ui/` or `ksx-studio/assets`, so a
phone lays the page out at a 980px virtual viewport and scales it down —
CSS breakpoints never fire, and no amount of media-query work will make them.
The viewport tag is the first line of this section's work, not a detail of it
(task #28). It is one line in the page head, and it belongs to whoever is next
in `render.rs`.

**ButtonCheck is not on Studio to be made responsive.** There is no live-input
channel in `ksx-studio` at all — no feed on `AppState`, no frame type, no
handler. The strongest argument in this section therefore rests on a capability
that exists only in the egui, and the first step is a backend-to-surface wiring
job, not a CSS pass. §2's build order already says which comes first.

Order follows that: viewport tag, then the live feed, then a responsive pass on
`/` and status, `/map` last. Mapping asks you to press the key it is capturing,
which a phone cannot do for a desk keyboard — so it is the least valuable page
on the smallest screen.

## §9 User flows worth writing down

Four journeys carry nearly all the product's surface area:

1. **First-time setup** — no config: find the board, name it, claim it, wire a
   slot, prove a button lights.
2. **Change a mapping** — running cabinet, one binding is wrong.
3. **"It doesn't work"** — the diagnostic path, which must terminate in a cause
   and not a shrug.
4. **Start a session** — the everyday path, and the one that must never need a
   keyboard.

Each should name the surface it happens on. Where a flow crosses surfaces, that
crossing is a design smell worth a second look.

## §10 What this settles for open work

- **Slot persona menu** — authoring, so Studio primary, egui view. Backend verb
  first: `SlotAssignRequest` carries slot, preset, profile and reload — **no
  persona** — so no surface can re-persona a slot until the wire type changes.
  A serde test pins that field set, so the day persona arrives on the wire the
  surface decision gets re-taken deliberately instead of by whoever adds the
  field.
- **Device pick UI** — Studio, following the existing CLI verb (§3). Also the
  egui: §3 row 3 no longer claims a view exists there.
- **Cabinet slot list scrolling** — egui, operating surface, still broken above
  four slots. The body *is* inside a `ScrollArea`; what is missing is any
  scroll-to-focus call, so the joystick can move the cursor to a row that is
  off-screen with no way to bring it into view (`nav.rs` moves the cursor with
  wraparound and no page-up, deliberately).
- **LAN + token + QR** — one coherent change-set, not three (§7), and the guard
  has two checks in it, not one.
- **Viewport meta tag** — one line, and the precondition for anything in §8.
