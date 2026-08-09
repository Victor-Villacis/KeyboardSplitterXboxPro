# Handoff

For whoever takes this over. It says what ksx is, how it is built, what is
finished, what is not, and — most usefully — **which beliefs about this codebase
turned out to be false**, because several of them cost a day each to discover.

Written 2026-08-09 at `v0.1.0`, the first release a stranger can install.

---

## §1 What ksx is

ksx splits one keyboard into as many as sixteen virtual game controllers on
Windows 11. Press `A` on an arcade panel and player 1's gamepad sees a button;
press `R` and player 2's does. That is the whole product, and everything below
serves it.

It began as a Rust rewrite of djlastnight's C# `KeyboardSplitterXbox`
(`docs/LEGACY-UPSTREAM.md`), which is why an importer exists for the old XML.

Two properties are contractual, and breaking either is a regression no matter
what else improves:

- **Fan-out.** One physical keyboard drives many slots with disjoint key sets.
  This is not a special case; it is the point.
- **The escape hatch.** LeftCtrl five times always stops emulation and gives
  every keyboard back. It lives in the capture thread and flips that thread's
  own passthrough, so no UI, no browser and no crashed process can take it away.

---

## §2 The shape of the code

15 crates. The dependency direction is the architecture, and it is worth
learning before touching anything:

```
ksx-core ────────► (nothing)      domain: keys, engine, personas, DeviceSelector
ksx-config ──────► core           TOML: config, games, presets, validation
ksx-api ─────────► core, config   THE WIRE CONTRACT between backend and surfaces
ksx-platform ────► core           Windows: USB enumeration, WinUSB claim/release
ksx-capture ─────► core, platform capture backends behind one trait
ksx-output ──────► core, hidmaestro  ViGEm pads, persona routing
ksx-backend ─────► all of the above  every verb's logic, the daemon, the supervisor
ksx-app ─────────► backend + surfaces  ONE FILE: clap definitions and a match
ksx-studio ──────► api, core, config   the browser UI
ksx-cabinet ─────► api                 the 10-foot egui panel
```

**`ksx-studio` and `ksx-cabinet` do not depend on the backend.** They reach it
only through `ksx-api` traits. That is what makes `docs/SURFACES.md` §1 — *the
backend owns state, every surface is a view* — a checkable property rather than
a slogan, and it is the single most important line in the graph.

`ksx-app` was 50,665 lines until 2026-08-09. It is now 3,798: clap derives and
one dispatch `match`. If you are looking for logic, it is in `ksx-backend`.

### The three surfaces, and why there are three

`docs/SURFACES.md` is the authority; the short version:

- **CLI** — every capability lives here first. It is the cheapest thing to test
  and the only one CI can drive headlessly. It is a **development surface**: the
  product does not require it (`docs/FIRST-RUN.md`).
- **Studio** (browser) — the workbench. Authoring: mapping, devices, profiles,
  the first-run flow. Better than immediate-mode GUI at a 25-binding preset.
- **egui cabinet panel** — the appliance. At a cabinet there is no mouse and no
  keyboard; the arcade panel *is* the input, and no browser UI can be driven by
  an arcade stick. That is why this surface cannot be deleted.

---

## §3 What is done

**Shipped and released as `v0.1.0`:** a Windows installer that puts an icon on
the desktop, installs the ViGEmBus driver from a checkbox, and hands off to the
app; `ksx open` starting the daemon and opening a chrome-less window; a
first-run flow that lists real devices by human names, stages a controller you
can change your mind about before anything is written or plugged, asks
split-or-freeze in the user's own words, and plays without saving; six Studio
pages; a live input feed with a button-check view; `ksx play` replaying a
recorded session; a unified USB+Bluetooth device list; the persona menu; 2,069
tests.

**Milestones:** M0–M3, M6.5, M7, M9, M10a are done. M4, M5, M6 are
code-complete and **cabinet-gate pending** (§4). M8 is a complete client blocked
on a driver that is not installable here. See `docs/ARCHITECTURE.md`.

---

## §4 What is not done — in priority order

### 1. GATE 3 — three milestones need one supervised evening

`docs/GATES.md` has the runbook. M4 needs its p99 measured during a real
4-player session; M5 needs GATE 1's Phase C (the frontend wrap); M6 needs a
session with **Interception uninstalled**, then a 14-day soak with zero recovery
actions. Nothing else can close these — they are measurements and a removal, not
code.

### 2. The first-run flow is 4 of 7 moments

`docs/FIRST-RUN.md` §1 numbers seven moments and §7 is the acceptance test: *a
person who has never seen ksx gets from a downloaded `.exe` to a controller
moving in a game, with no terminal, no file editing, and nobody telling them
what to do next.* Moments 2, 3, 4, 5 pass. The gaps:

- **Moment 6, the per-key half.** A staged slot takes a whole in-box layout in
  one click, and that works. But `/map` accepts `?slot=` and `?preset=` only —
  no staged target — so changing one button, or writing a macro, means Save
  first and then the mapper. `StageEdit::SetBindings` exists in `ksx-api` and no
  route sends it. The finished shape is one target field on the mapper's
  existing writes; **do not build a second mapper** (`docs/MAPPER-UX.md` is
  ~23k lines of finished UX).
- **Moment 7, unverified.** The software path is complete and the Guide binding
  now exists on the default layout, but nobody has walked it end to end on a
  machine that did not already have ViGEmBus.
- `ksx open` lands on `/` (status), not `/start`. A first-time user has to
  notice a nav link.

### 3. LAN access + pairing token + QR (task #23)

Studio binds `127.0.0.1` and refuses otherwise. The intent is recorded in `ksx
studio --help`. **It needs its own security review, and the live feed changed
the threat model**: an unauthenticated stream of a user's keystrokes on the LAN
is a different problem than an unauthenticated config page. Note `guard.rs`
deliberately allows a request with no `Origin` header — correct on loopback,
insufficient on a LAN, because `curl` sends none either.

### 4. Smaller, tracked

Build B (the setup wizard) and Build C's polish in `docs/MAPPER-UX.md`; the
any-HID-as-input easter egg; code signing (an unsigned installer throws
SmartScreen at every new user — Azure Trusted Signing is ~$10/month).

---

## §5 Things that are true and surprising

Each of these was believed otherwise, and each cost real time.

**A device path is not always port-derived.** Windows keys a devnode off the USB
**serial** when the device reports one, and off the socket only when it does
not. Measured on the cabinet: an I-PAC 4X's instance path survived a move from
root port 5 to port 7 **byte for byte**. `DEVICE-IDENTITY.md` §1 asserted the
opposite and three code sites cited it as authority. ksx cannot tell which kind
of path it holds once the board is absent — a serial lives in the descriptor of
a device that is not there — so **no message may assert that a board moved**.

**Two identical boards cannot both be claimed.** An INF binds by hardware id,
which twins share, so claiming one would claim both. `winusb.rs` refuses
`SharedHardwareId` for both while both are plugged. The port rung tells them
apart in *config*; the claim then declines to act on it. Do not read
"twins work" — read "twins are never silently confused".

**Bluetooth keyboards can be split but never WinUSB-claimed.** They enumerate
under `BTHENUM\`, not `USB\`; WinUSB binds a USB interface and there is none.
Interception sees them fine. Also: a paired-but-disconnected BT keyboard reads
as *present* all day, so it is listed but excluded from last-keyboard
arithmetic — otherwise someone reads "2 keyboards", claims their panel, and is
locked out by a keyboard in a drawer with dead batteries.

**A failed read is not an absence.** "I could not enumerate" and "you have no
devices" are different sentences and users act on them differently. This
project's signature bug is reporting success while the panel is dead — a session
once read healthy because a WinUSB board had silently fallen back to
Interception. `SURFACES.md` §1b.

**`ksx-app`'s test target loses `fn main` as a liveness root** (rustc 1.97), so
the whole runtime-only chain reads as dead code there and nowhere else. Hence
`#![cfg_attr(test, allow(dead_code))]`, with the bin target remaining the
authority.

---

## §6 How to work on it without losing a day

**Read `CLAUDE.md` first.** It is the map — crate layout, the shapes to copy,
every landmine. It exists because agents were re-reading the whole codebase to
orient, at enormous cost.

**Push your branch; let CI gate it.** `rust-toolchain.toml` pins 1.97.1 and CI
runs on every branch: fmt, workspace clippy, **all four feature combinations**,
the full suite, plus the installer compile. Do not run the four-way matrix
locally.

> **The dev machine's CPU is failing.** An i9-14900K that crashes rustc
> non-deterministically — `STATUS_ACCESS_VIOLATION` / `STATUS_STACK_BUFFER_OVERRUN`
> on byte-identical input, at Intel defaults with XMP and AI Boost off and
> microcode 0x12F. **That is hardware, not your code**; retry up to 3×. It is
> why shipped binaries are built on CI, and it is under RMA. If you are on
> different hardware, ignore this — but leave the retry advice in place.

**The four feature combinations are not paranoia.** `studio` and `cabinet` are
independent opt-ins, the default build compiles neither, and five breakages have
reached master through that gap.

**Never hand-merge generated assets** (`crates/ksx-studio/assets/*`). Regenerate
with `cd studio-ui && node build.mjs`. They are `-text` in `.gitattributes`, so
a clean rebuild leaves `git status` clean — if it does not, something really
changed. A hand-resolved manifest yields a page whose HTML and JS disagree, and
that fails in a browser and in no Rust test.

**Doc section numbers are load-bearing.** ~30 code sites cite
`DEVICE-IDENTITY.md` by §number, and `crates/ksx-app/tests/docs.rs` fails the
build if a cited section stops existing.

**A test must fail against the broken version**, and say in a comment which one.
Two tests were found passing by coincidence in a single evening — both asserted
a literal where they meant "the default", and held only while those happened to
be the same string.

---

## §7 Releasing

`docs/RELEASING.md` is the runbook. Short version: a **CLI-pushed tag** is the
trigger — `git tag v0.1.1 && git push origin v0.1.1`. A tag pushed from inside
Actions does not fire workflows, so it must come from a person's machine. CI
builds on a clean runner, re-hashes the installer, refuses to publish if the
hash disagrees, and attaches it to a GitHub Release with the SHA-256 and source
commit in the notes.

`Cargo.toml` and `packaging/ksx.iss` both carry the version and the release
**fails** if they disagree — deliberately, rather than patching one, because
`AppVersion` is also `VersionInfoVersion` and the Apps & Features row.

---

## §8 The map of the docs

| you want | read |
|---|---|
| where things are, what will bite you | `CLAUDE.md` (repo root) |
| the milestone table and the pipeline | `ARCHITECTURE.md` |
| which surface a capability belongs on | `SURFACES.md` |
| the customer's journey, as a spec | `FIRST-RUN.md` |
| how a device is identified, and why not by path | `DEVICE-IDENTITY.md` |
| what each control surface can do | `CONTROL-SURFACE.md` |
| keys, chords, turbo, SOCD, macros | `INPUT-TRANSFORMS.md` |
| supervised hardware runbooks | `GATES.md` |
| the panel is dead / a claim went wrong | `RECOVERY.md` |
| driver policy: pins, signatures, consent | `DRIVERS.md` |
| the mapper's UX spec and its unbuilt halves | `MAPPER-UX.md` |
| Studio's visual language | `DESIGN-SYSTEM.md` |
| why there is no native config UI | `M9-DECISION.md` |
| the idea/enhancement ledger | `ENHANCEMENTS.md` |
| which topologies are proven vs untested | `USE-CASES.md` |
| cutting a release | `RELEASING.md` |
| the C# app this replaced | `LEGACY-UPSTREAM.md` |

`docs/research/` holds dated investigations. **Do not "correct" them** — a
research file edited to agree with a later decision stops being evidence of
anything.

---

## §9 The one thing to keep

If everything else here is rewritten, keep this: **do not let a screen report
success it cannot verify.** Every serious defect in this project's history has
that shape — the session that read healthy while the panel was dead, the
refusal that asserted a port move it could not know about, the page that said
"no devices" when it meant "I could not look", the test that passed against a
coincidence. The tests, the doc citations, the parity guard and the staged-setup
type all exist to make that failure mode expensive.

A user standing at a cabinet with a dead panel and a green status screen has
been failed worse than one who got an error.
