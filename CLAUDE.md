# ksx — orientation

Read this before touching anything. It is the map, not the reasoning: it says
**where things are and what will bite you**. The `docs/` files say *why*, and
this file points at the right one instead of repeating it.

ksx splits one keyboard (an arcade encoder — an I-PAC) into up to 16 virtual
gamepads on Windows 11. Rust, workspace of 13 crates + one vendored dep.

## The one rule everything else follows

**The backend owns state; every surface is a view** (`docs/SURFACES.md` §1).

A capability becomes a *typed spec in, pure plan out* in the backend; the CLI,
the egui cabinet panel and Studio (browser) each call it and render the result.
No logic in a surface. A constant like `MAX_SLOTS` lives in `ksx-core` **only**
and is served to surfaces — a number hardcoded in TypeScript is the specific
bug this rule exists for.

Build order for any new capability: **backend verb → CLI → the surface a human
performs it on**. There is no "egui first or web first" question.

## Where things are

| you want | it is here |
|---|---|
| domain model, engine, keys, personas, `MAX_SLOTS`, `DeviceSelector` | `crates/ksx-core` |
| config + games + presets TOML, validation | `crates/ksx-config` |
| capture backends (Interception, WinUSB) behind `CaptureBackend` | `crates/ksx-capture` |
| ViGEm pad output, persona routing | `crates/ksx-output` |
| Windows plumbing: USB enumeration, WinUSB claim/release | `crates/ksx-platform` |
| the wire contract between backend and every surface | `crates/ksx-api` |
| CLI verbs, daemon, tray, session supervisor, writers | `crates/ksx-app` |
| the browser UI (Rust render seams + routes) | `crates/ksx-studio` |
| the browser UI's TypeScript islands | `studio-ui/src` |
| the 10-foot cabinet panel (egui) | `crates/ksx-cabinet` |

`crates/ksx-app/src/` is the biggest surface area. Orient by verb: `main.rs`
registers every CLI command; `device_edit.rs` / `device_scan.rs` are the device
picker's write and read halves; `run/` is the session supervisor (`plan.rs`
builds a plan, `resolve.rs` turns config spellings into live devnodes);
`daemon/` is the resident tray process and its control pipe; `sources.rs` is
where surfaces get their data.

## Adding things — the shapes to copy

**A CLI verb**: a typed spec in, a pure plan out, a timestamped backup before
any write, the store's atomic save doing the I/O. Copy `device_edit.rs` — its
module docs state the pattern. Refusals carry a stable `code()` and an
`advice()` that names a command that actually exists.

**A Studio page**: routes go in the ONE `Router::new()` chain in
`ksx-studio/src/server.rs`, **before** the `.layer()` guard — anything after it
is unguarded. Every mutating route obeys `guard.rs` (CSRF/DNS-rebinding) and
303-redirects with the outcome in `?flash=`. A page is four seams (scalars,
lists, shows, `build_slots`) plus one layout test — copy `render_devices.rs`,
and read its top comment about what must never come back into a view. The Rust
seam and the TypeScript island are mirrors: every string composed in Rust must
be composed identically in TS, and the layout test pins the Rust side.

**A cabinet screen**: `ksx-cabinet/src/nav.rs` lists them. Remember there is no
mouse and no keyboard at a cabinet — the arcade panel is the input, so anything
you add must be panel-navigable.

## The gate — run all of it before you commit

```
cargo fmt --check                       # on the crates you touched
cargo clippy --workspace --exclude vigem-client --all-targets -- -D warnings
cargo clippy -p ksx-app --all-targets -- -D warnings                       # no features
cargo clippy -p ksx-app --all-targets --features studio -- -D warnings
cargo clippy -p ksx-app --all-targets --features cabinet -- -D warnings
cargo clippy -p ksx-app --all-targets --features studio,cabinet -- -D warnings
cargo test --workspace --exclude vigem-client
```

The four feature combinations are not paranoia: `studio` and `cabinet` are
independent opt-ins, so the default build compiles neither, and **five separate
breakages have reached master through that gap**. `--features studio` alone has
caught dead code twice.

Touched `studio-ui/`? Also `cd studio-ui && node build.mjs`, commit the
regenerated assets, and confirm a fresh rebuild is byte-identical.

## Landmines — each one has already cost a day

- **This machine's CPU is failing.** rustc dies non-deterministically with
  `STATUS_ACCESS_VIOLATION` / `STATUS_STACK_BUFFER_OVERRUN` on byte-identical
  input. That is hardware, **not your code** — retry up to 3× before concluding
  anything. Shipped binaries are built on CI for this reason.
- **The toolchain is pinned** (`rust-toolchain.toml`, 1.97.1). Never
  `rustup override`. Local 1.96 vs CI 1.97 once made "clippy clean" mean two
  different things and shipped 24 diagnostics to CI.
- **Never hand-merge generated assets** (`ksx-studio/assets/*`, `manifest.json`,
  `sw.js`, hashed bundles). Regenerate from `studio-ui/`. A hand-resolved
  manifest yields a page whose HTML and JS disagree — it fails in a browser and
  in no Rust test.
- **Never assert what the code cannot know.** A refusal once said "this id names
  one specific USB SOCKET"; a live port move disproved it, because Windows keys
  a devnode off the serial when the board reports one. State what *decides* the
  behaviour, not what you assume follows.
- **A failed read is not an absence.** "I could not read this" and "there is
  nothing here" are different sentences and users act on them differently. This
  project's signature bug is reporting success while the panel is dead.
- **Doc section numbers are load-bearing.** ~25 code sites cite
  `DEVICE-IDENTITY.md` by §number; `crates/ksx-app/tests/docs.rs` fails the
  build if a cited section stops existing. Renumbering breaks all of them.
- **`.iss` files**: never write a Pascal `{ }` comment containing `{app}` —
  Inno ends the comment at the first `}`. Use `//`. This broke the installer.
- **Windows/CRLF**: `include_str!` reads files as checked out. A test comparing
  against `\n` passes locally and fails on every fresh clone. CI was red 57 runs
  over this.

## Tests

A test must **fail against the broken version**. Name in a comment which broken
version it catches. A test that re-encodes the implementation is worse than
none — several have been deleted for this. Hardware-touching tests live behind
the `cab-tests` feature and never run in CI.

`crates/ksx-app/tests/` holds the cross-cutting ones: `docs.rs` (doc citations
stay true), `replay.rs` (a real 392-event cabinet recording drives the engine
offline — the regression oracle for the whole input path).

## Which doc to open

| question | doc |
|---|---|
| which surface does this belong on? | `docs/SURFACES.md` |
| how is a device identified, and why not by path? | `docs/DEVICE-IDENTITY.md` |
| what can each control surface do? | `docs/CONTROL-SURFACE.md` |
| the milestone map and exit criteria | `docs/ARCHITECTURE.md` |
| supervised cabinet runbooks (the hardware gates) | `docs/GATES.md` |
| the panel is dead / a claim went wrong | `docs/RECOVERY.md` |
| keys, chords, turbo, SOCD, macros | `docs/INPUT-TRANSFORMS.md` |
| Studio's visual language | `docs/DESIGN-SYSTEM.md` |
| why there is no native config UI | `docs/M9-DECISION.md` |
| the enhancement/idea ledger | `docs/ENHANCEMENTS.md` |

## Working style here

Commit early and often — several small commits beat one perfect one that never
lands. Write commit messages that explain **why**, in the repo's voice: read
`git log` before writing one. Never push unless asked.
