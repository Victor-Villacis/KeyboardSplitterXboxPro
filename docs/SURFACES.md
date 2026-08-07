# Surfaces: what each one is for

ksx has three faces — the CLI, the egui cabinet app, and Studio in a browser —
and one backend. This file records which face owns what, in what order they get
built, and the alternatives that were considered and rejected. It exists so that
"which surface does this go on?" is answered once instead of re-argued per
feature.

Written 2026-08-07, after `ksx device pick` shipped as a verb with no web face
and the question came up for the fourth time.

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

The cost of breaking this rule is not theoretical. `MachineSource::devices()`
sat with **no implementation** while the cabinet UI had a devices screen — the
screen could not list a single board, and no amount of UI work would have fixed
it. That is what a surface owning something the backend does not have looks like.

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
| Edit config, profiles | owns | — | **primary** |
| Device pick / remove | owns | view | should follow |
| WinUSB claim / release | owns | view | never (needs elevation) |
| "Press a button, see it light" | — | **primary** | useful on phone (§6) |
| Is it working: pads, drivers | owns | **primary** | view |
| Start / stop / switch profile | owns | **primary** | convenience |

"owns" = the verb lives here. "primary" = where a human does it. "view" =
renders backend state, takes no decisions.

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
two routes, `/` and `/map`.

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
- **`ksx-studio/src/guard.rs` will reject it.** The DNS-rebinding defence checks
  the `Host` header **by name** against loopback. A LAN address fails that check.
  The allow-list has to learn the bound address in the same change-set, or our
  own guard becomes the bug report.

## §8 Mobile: responsive-only, aimed at diagnostics

No dedicated touch layout yet, and no deferring mobile either — because there is
one phone use case that beats every other surface:

**You are behind the cabinet with the panel open, pressing buttons, and the
phone in your hand shows which key fired.** That is ButtonCheck on a phone, and
it is better than walking round to the monitor for every wire.

Order follows that: responsive pass on `/` and status first, `/map` last.
Mapping asks you to press the key it is capturing, which a phone cannot do for a
desk keyboard — so it is the least valuable page on the smallest screen.

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
  first: `SlotAssignRequest` carries preset only, no persona, so no surface can
  re-persona a slot until the wire type changes.
- **Device pick UI** — Studio, following the existing CLI verb (§3).
- **Cabinet slot list scrolling** — egui, operating surface, still broken above
  four slots.
- **LAN + token + QR** — one coherent change-set, not three (§7).
