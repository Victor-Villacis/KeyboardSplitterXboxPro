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
guess.

That was the surface-parity guard on the task list (#26), and it now exists:
`crates/ksx-app/tests/parity.rs` walks the clap command tree, reads Studio's
routes out of `Router::new()` and the cabinet's screens and `Ask` variants out
of its source, and asks of every cell in §3 whether the tree agrees. A verb no
row names and no exemption covers fails it. So does a cell claiming a face that
is not there — and so does the opposite, which is what it found on the day it
was written: two cells still saying `planned` about pages that had shipped.
What it cannot check is a capability nobody wrote a row for, which is the
remaining reason to keep §3 current by hand.

### §1a Rendered COPY is logic too, and one page proves it

A Studio island seeds its signals from the server's paint and then rewrites the
same signals from a 2 s poll. It is very easy to let the island compose its own
sentences from the polled data, and every one of them is then a second
implementation of a backend rule in another language.

The Profiles page shipped that way and review found the drift immediately: the
slot-count input's ceiling was `createSignal("16")` in `ProfilesIsland.ts`, its
setter was never called, and no payload field could reach it. `MAX_SLOTS` has
already been raised once (task #17). The next raise would have had the server
render `max="32"` and hydration write `16` straight back over it — a *legal*
input silently refused, for the same reason `main.rs`'s `slot_arg` module
exists. The summary lines, the pill mapping and the row text were duplicated the
same way; they had not drifted yet.

The shape that fixes it: **one serialized derived block**. `ProfilesDerived`
(`crates/ksx-studio/src/snapshot.rs`) holds every displayed string, every count,
both numeric ceilings and every `show:` boolean, computed once from the provider
data; `render_profiles.rs` injects it into the FMIR slots and `applyProfiles`
assigns it to signals. Neither composes anything. A new page copies this, not
the two-halves version.

### §1b A refused READ is not an empty result

"I could not read this" and "there is nothing here" are different sentences, and
a user acts on them differently: one says *go fix your config*, the other says
*go make your first profile*. A surface that renders the second when the first
is true has reported success over a read that did not happen — which is the
failure mode this project keeps hitting (the session that read as healthy while
the arcade panel was dead, because a WinUSB board had fallen back to
Interception).

The rule: **an `Err` from a provider gets a typed field on the payload, never a
`Default::default()` view.** Substituting a default is what turns a refusal into
a count of zero, and a count of zero into a confident wrong sentence. See
`ProfilesPayload::profiles_error` / `presets_error`, and note the second-order
bug the Profiles page had: a defaulted `PresetsView` set `noPresetsYet`, whose
copy points at a template form fed by *the same read that just failed* — a
closed loop with a wrong sentence on it.

A page that gets this right needs a test that fails when the two are conflated;
asserting the failure state renders is not enough, because that passes while the
absence sentence renders too.

## §2 Build order

1. **Backend verb** — typed spec, pure plan, tested against synthetic fixtures.
2. **CLI** — the cheapest surface to test and the one CI can drive headlessly.
3. **The surface the task is actually performed on** (§3, §4).

There is no "egui first or web first" question. That framing assumes a surface
owns a capability, which §1 forbids. The real question is only ever *which
surface does a human perform this task on*, and that is answered by the matrix.

## §3 The capability matrix

> **This table is a test.** `crates/ksx-app/tests/parity.rs` parses it and
> checks every cell against the tree — the clap command tree, Studio's routes,
> the cabinet's screens and `Ask` variants. Editing a row means editing that
> test's anchors too, and a word the guard has not been taught (the vocabulary
> is below) fails it rather than passing unread. Adding a ROW with no anchors
> also fails: an unbound row is checked by nothing, which is the state the whole
> table was in before the guard existed.

| Capability | CLI | egui (cabinet) | Studio (browser) |
|---|---|---|---|
| Author presets / key mappings | owns | — | **primary** |
| Edit config, profiles | owns | slot→preset only | **primary** |
| Device pick / remove | owns | planned | **primary** |
| WinUSB claim / release | owns | planned | never (needs elevation) |
| "Press a button, see it light" | input only (`ksx monitor`) | **primary** | planned (§8) |
| Is it working: pads, drivers | owns | **primary** | view |
| Spawn test pads / prune the bus | owns | — | **primary** (§3a) |
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
- **Edit config — Studio.** *Superseded 2026-08-08; see below.* Was
  "**primary**". At the audit no Studio route wrote `config.toml` or
  `games.toml`; the only config-adjacent route re-read. Every `/map/*` write
  went to a preset file. `AppState` held no `MachineSource`, and `ControlSource`
  had exactly one config-writing verb (`slot-assign`) which Studio never called.
  It was the right destination, so it stayed in the table — as a plan.
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

Two more were corrected on 2026-08-08, and this time by the guard rather than
by a person reading the table — both in the direction that is cheaper to make
and harder to notice, a face that SHIPPED while the cell still said `planned`:

- **Edit config — Studio.** Was "planned primary". `/setup` and `/profiles`
  have since shipped and the plan is the surface: `/setup/import` rewrites the
  whole config root, `/setup/slot` posts the same `ControlSource::assign_slot`
  that `ksx slot assign` performs, and `/profiles/new` writes a games.toml
  profile through `MachineSource::profile_new`. **primary**, which is where the
  bullet above already said it belonged.
- **Device pick / remove — Studio.** Was "planned (#22)"; #22 shipped
  `/devices`, `/devices/pick` and `/devices/remove`. The egui half stays planned
  and drops the issue number, because #22 was never about the cabinet — its five
  screens are still ButtonCheck, Status, Session, Profiles, Presets.

### §3a Why the pad verbs get a Studio face and WinUSB claim does not

Both are driver operations and only one of them is dangerous, so "it touches a
driver" is not the line. The two that matter are:

- **Can it lock the user out of the machine?** A WinUSB claim takes a keyboard
  out of the keyboard stack, and the worst case is a panel that no longer types
  and a user who cannot type the command to undo it. A test pad plugs and
  unplugs; the worst case is four pads a game cannot see, which the page says
  out loud before the click.
- **Can the surface state the consequence in advance?** `ksx pads --count 8
  --persona xbox360` plugs eight pads and Windows hands four of them to nobody.
  That is the exact failure a web page is *better* at than a console: the
  option can carry its own label. The backend composes it
  (`MachineSource::pads_view`); the page renders it. The console says the same
  thing from the same constant before it plugs (`pads::ceiling_warning`, task
  #16) — a warning, not a refusal, because plugging dead pads on purpose is a
  legitimate thing to ask a *test* command for. The configuration layer is
  where that ceiling refuses (`ksx_config::Issue::TooManyXinputSlots`), and the
  split is the general rule: a test verb warns, a verb that persists refuses.

Elevation does not change the answer either — it changes the wording. A prune
restarts a bus devnode, which needs an administrator token, and ksx never
self-elevates. So Studio SAYS so before the click (`PadsView::elevated`) and
the backend refuses with the elevated command attached. That is a better
outcome than the surface pretending the verb does not exist.

Two rules fell out of building it, and both generalise past this page.

**A verb a surface can repeat needs a total, not a per-call bound.** A console
operator watches the pads accumulate and stops; a button does not. `ksx pads
--count 16` five times over leaves eighty pads on the bus, and the recovery —
a prune — is refused to the unelevated process a browser-launched Studio
usually is. So the bound is on the state (`SpawnPlan::BusFull`), and the menu
stops offering what the plan would refuse.

**A read that FAILED is not a reading of nothing.** "I could not look" and
"there is nothing there" are different sentences and a user acts on them
differently: the first sends them to `ksx doctor`, the second tells them the
machine is fine. So an unanswerable count is `Option<u8>` all the way to the
page rather than a `0` some layer invented, and a refused
`MachineSource::pads_view` renders `PadsView::unreadable` — every composed
sentence saying the read failed — instead of a default view whose devnode line
asserts a driver is not installed and whose empty pad grid draws a clean bus.
This is the shape of the failure that once let a session report success while
the arcade panel was dead.

What is unchanged: the dry-run-first consent shape is the backend's, not the
surface's. `pads_prune(confirm)` is `--yes`, spelled the same way and refusing
the same things, and a POST that did not come from the confirm screen gets the
dry run.

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
returns `NonLoopbackBind` rather than serving a LAN address. Its **pages** are
`/` (status), `/map` (the mapper), `/check` (the button check), `/pads` (the
ViGEm bus and its two verbs), `/devices` (the picker), `/profiles` (profiles &
presets) and `/setup` (the configuration).

`/check` is the one page that performs no verb at all, and the one fed by a
channel that is not the control pipe: `GET /api/live` is Server-Sent Events
over the daemon's own outbound-only feed pipe, which is what lets a press on
the panel light a control in a browser at display rate. It is still a VIEW —
it writes nothing, and it decides nothing, because its whole control roster is
`MapperSlot::bindings`' key set arriving from the backend (§1).

Seven pages, dozens of routes: the rest are the `/api/*` reads, the mutating
form endpoints, the service worker, the asset handler and three icons. The
distinction is not pedantry — it is the whole reason the CSRF guard is one
layer over the router rather than a check per handler, because "the mapper
alone grew eight form endpoints in three milestones" and the failure mode
being prevented is forgetting one.

`/setup` is where the config itself lives, and it has exactly **two verbs**:
Export downloads the whole root as one JSON document, Import pastes one back
(dry run unless the write box is ticked — `ksx config import`'s consent shape,
unchanged). Neither takes a path: `MachineSource::config_export|config_import`
are in-memory on purpose, because a person who asked a page for their
configuration should not be handed a directory to go and find. A config root
appears on that page once, in small print, for a bug report to quote.

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

Two corrections to how that reads — the first of which was itself wrong, and
is kept because *how* it was wrong is this repo's most instructive audit
failure to date:

**The viewport tag was never missing.** An earlier revision of this section
said "responsive-only currently means not responsive at all: there is no
`<meta name="viewport">` anywhere in `studio-ui/` or `ksx-studio/assets`", a
task was filed to add the one line, and the line was added — producing a
DUPLICATE, because `forma-server`'s own template (`template.rs`, 0.1.4 and
0.2.0 alike) has emitted the tag on every page this crate ever rendered.
Three separate greps reached the same false conclusion the same way: they
searched the page's *source*, and the head of these pages is assembled by a
dependency, so the truth was only ever in the *output*. The claim is now
pinned where the truth lives — `render.rs::assert_complete_head` reads the
rendered HTML and asserts the tag is present, in `<head>`, exactly once —
and the lesson generalises: **an audit of a claim about output must read the
output.** Breakpoints and media queries therefore already fire on phones;
what they fire *against* is layouts nobody has tuned, which is the actual
work (task #24).

**ButtonCheck is not on Studio to be made responsive.** There is no live-input
channel in `ksx-studio` at all — no feed on `AppState`, no frame type, no
handler. The strongest argument in this section therefore rests on a capability
that exists only in the egui, and the first step is a backend-to-surface wiring
job, not a CSS pass. §2's build order already says which comes first.

Order follows that: the live feed, then a responsive pass on `/` and status,
`/map` last. Mapping asks you to press the key it is capturing, which a phone
cannot do for a desk keyboard — so it is the least valuable page on the
smallest screen.

## §9 User flows worth writing down

Four journeys carry nearly all the product's surface area:

1. **First-time setup** — no config: find the board, name it, claim it, wire a
   slot, prove a button lights. **Studio's `/setup`** (§5): the checklist is
   decided in the backend (`ksx-app::onboard::plan_steps`, pure) and rendered,
   never re-derived per surface; each step is one backend verb, and the board
   step LINKS to the devices screen instead of duplicating it. Every step is
   resumable because none of them is a wizard step: each reads the config as it
   stands and writes one complete thing, so an abandoned run leaves a valid
   config rather than a half-written one.
2. **Change a mapping** — running cabinet, one binding is wrong.
3. **"It doesn't work"** — the diagnostic path, which must terminate in a cause
   and not a shrug.
4. **Start a session** — the everyday path, and the one that must never need a
   keyboard.

Each should name the surface it happens on. Where a flow crosses surfaces, that
crossing is a design smell worth a second look.

## §10 What this settles for open work

- **Slot persona menu** — **settled and built, 2026-08-08 (task #8).** The
  decision stands as it was written: authoring, so Studio primary, egui view.

  The wire type changed, which is what this entry was waiting for.
  `SlotAssignRequest` now carries `persona` as an **optional string**, and both
  halves of that are load-bearing. *Optional*, because `Persona::default()` is
  `xbox360`: a defaulted field would have read every pre-2026-08-08 request as
  "make this slot an Xbox 360 pad" and silently un-PlayStation-ed slots 5–8 the
  first time somebody re-pointed a preset. *A string*, because ksx-core carries
  no serde and the alias table lives in one `FromStr` — a surface must never
  hold a copy of it to fill this field. The serde test that pinned the old
  field set still pins the new one; it was renamed and re-argued, not deleted.

  What each surface got, and why:

  - **Backend** (`ksx-app/src/slots.rs`) applies it, and refuses two things in
    words: a persona this build cannot plug (`Persona::can_plug`, which reads
    the backend's `is_implemented` and never a driver probe) and a fifth XInput
    slot — counted **after** the write would land, over the whole destination
    file, so the refusal is about the config that would exist rather than about
    the one field being touched.
  - **CLI** — `ksx slot assign --slot N --persona P`, lenient parsing through
    the same `FromStr`. Preset and persona are independently optional: either,
    both, or the preset alone.
  - **Studio** — the picker sits on `/setup`'s "Wire a slot" form, beside the
    slot and preset selects that already POST `slot-assign`. **Not `/profiles`,
    which has no slot rows**: a second slot editor on a second page would be
    two front doors onto one verb, which is the drift §1 forbids. The option
    list is `SetupView::personas`, served by the backend with a `can_plug` flag
    and a `why_not` sentence per entry; nothing about personas is spelled in
    TypeScript.
  - **egui** — renders the persona in the Presets screen's slot rows. No
    picker: §4's rule is that anything needing text entry or a menu of five
    belongs elsewhere, and re-personaing is a between-sessions authoring act,
    not something done standing at the cabinet mid-evening.
- **Device pick UI** — Studio, following the existing CLI verb (§3). Also the
  egui: §3 row 3 no longer claims a view exists there. `/setup`'s first step
  links to `/devices` rather than growing a second picker.
- **`ksx games new` — the CLI half of profile creation, owed.** Studio's
  `/profiles` page creates a games.toml profile through
  `MachineSource::profile_new` over a pure plan in `ksx-app`'s `profile_edit`
  — but there is no CLI verb for it, so §2's build order ran 1 → 3 with 2
  skipped. That is backwards and it shows: `profile_edit` is gated
  `#[cfg(any(feature = "studio", feature = "cabinet"))]` because Studio is its
  only caller, which is a backend module whose existence depends on a UI
  feature flag. `plan_new` / `apply_new` are pure and already carry the
  refusals; the CLI verb is a thin driver over them, and it removes the gate.
  Until then §3's "Edit config, profiles | CLI owns" row is aspirational for
  the CREATE half.
- **Cabinet slot list scrolling** — egui, operating surface, still broken above
  four slots. The body *is* inside a `ScrollArea`; what is missing is any
  scroll-to-focus call, so the joystick can move the cursor to a row that is
  off-screen with no way to bring it into view (`nav.rs` moves the cursor with
  wraparound and no page-up, deliberately).
- **LAN + token + QR** — one coherent change-set, not three (§7), and the guard
  has two checks in it, not one.
- **Viewport meta tag** — one line, and the precondition for anything in §8.
