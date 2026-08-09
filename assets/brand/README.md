# The ksx mark

Half a keyboard on the left; two pads and two buttons spilling off the right,
each overlapping the one before it. One input becoming four players — which is
the entire product in one picture.

Palette is Street Fighter's, and it is **locked**:

| token | hex | what it is |
|---|---|---|
| `PLATE` | `#331a4d` | the rounded plate everything sits on |
| `OUTLINE` | `#140a1f` | every outline in the detailed mark |
| `KB` | `#f4f1e6` | keyboard case |
| `BEZEL` | `#d8d2bd` | the recess inside the case |
| `KEY` | `#463456` | keycaps and the spacebar rail |
| `C1` | `#e03c31` | player 1 — red |
| `C2` | `#3d8de8` | player 2 — blue |
| `C3` | `#3fae52` | player 3 — green |
| `C4` | `#f5b81c` | player 4 — yellow |

## Regenerate everything

```
cargo run --manifest-path tools/icongen/Cargo.toml --release
```

That is the whole procedure. It reads the two masters below and rewrites every
generated file in one pass, so the shell icon, the favicon, the installer icon
and the cabinet window icon cannot disagree about what ksx looks like.

`tools/icongen` is its **own cargo workspace** on purpose — resvg's ~70-crate
tree stays out of ksx's `Cargo.lock` and out of every `clippy --workspace`
run. Building ksx never runs it; the rasters are committed exactly like
`crates/ksx-studio/assets/`.

## Two drawings, not one drawing at eight sizes

This is the point of the whole set, and the reason the `.ico` is worth
producing at all.

| file | used at | what is different |
|---|---|---|
| `ksx-detailed.svg` | **48 px and up** | outlines, the bezel, five key rows, the real gamepad silhouettes, specular highlights on the buttons |
| `ksx-simple.svg` | **32 px and below** | no outline anywhere, no bezel, four fatter key rows, pads reduced to plain rounded rects |

At 16 px the detailed mark's 2.4-unit outline is 0.6 px wide — not a line, a
grey smear over every edge — and its 7.6-unit keycaps are under 2 px tall.
The simplified drawing throws away everything that cannot survive and keeps
the two things that still read at that size: the keyboard silhouette, and two
overlapping pads.

Windows does the handoff for you *if you ask it properly*: `LoadImageW` with
an explicit size picks the matching `.ico` entry, which is why
`daemon/tray.rs` uses that and not `LoadIconW`.

## What lives where

**Masters — hand-edited, signed off. Do not redesign.**

| file | notes |
|---|---|
| `ksx-detailed.svg` | contains Lucide's `gamepad-2` path (ISC) — see `/NOTICE` |
| `ksx-simple.svg` | no third-party geometry |

**`dist/` — generated. Never hand-edit; the tool overwrites it.**

| file | size | art | consumer |
|---|---|---|---|
| `ksx.ico` | 16, 20, 24, 32, 48, 64, 128, 256 | per the table above | `crates/ksx-app/build.rs` → `ksx.exe` resource 1 → Explorer, taskbar, UAC, tray; `packaging/ksx.iss` → `SetupIconFile` |
| `ksx-16 … ksx-256.png` | one each | per the table above | the `.ico`'s source images; also the files to look at when reviewing the handoff |
| `apple-touch-icon.png` | 180 | detailed | iOS home screen. Flattened onto `PLATE` — iOS composites this over black and applies its own mask, so transparent corners would show as black wedges |
| `ksx-256.rgba` | 256 | detailed | `crates/ksx-cabinet` window icon: raw straight-alpha RGBA8, so the cabinet needs no PNG decoder and keeps its two-dependency boundary |

**`crates/ksx-studio/brand/` — also generated, by the same run.**

`favicon.ico` (a copy of `dist/ksx.ico`), `favicon.svg` (a copy of
`ksx-simple.svg`), `apple-touch-icon.png`. It is a separate folder from
`crates/ksx-studio/assets/` because that one is the output directory of
`studio-ui/build.mjs`, which `rmSync`s it at the top of every run — icons
parked there would vanish at the next UI rebuild and 404 with nothing in any
log to say why.

`favicon.svg` is the **simplified** master, deliberately: browsers that
support SVG icons prefer them over the `.ico`, and what they do with one is
render it into a 16–32 px tab.

Copies drift, so both are pinned by tests
(`crates/ksx-studio/src/render.rs::brand_embed_carries_the_trio` compares
bytes; `crates/ksx-cabinet/src/lib.rs` has a `const` assertion on the blob's
length).

## Where the mark is wired

| surface | wiring |
|---|---|
| `ksx.exe` icon + version tab | `crates/ksx-app/build.rs` (winresource → `rc.exe`) |
| tray notification icon | `crates/ksx-backend/src/daemon/tray.rs::load_tray_icon` |
| Studio favicon / SVG icon / apple-touch | `crates/ksx-studio/src/render.rs` (`BrandAssets`, `ICON_LINKS`) + `server.rs` root routes |
| egui cabinet window | `crates/ksx-cabinet/src/lib.rs::launch` |
| installer + uninstaller | `packaging/ksx.iss` |
