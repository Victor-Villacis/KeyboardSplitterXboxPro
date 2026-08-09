# The legacy C# app — where it lives, and how to read a citation

ksx is a ground-up Rust rebuild of djlastnight's **KeyboardSplitterXbox** (2016,
C#/.NET 4.0, unmaintained). Around twenty doc comments across `crates/` cite a
`.cs` file by path to explain why the Rust does something odd — why a v2 preset
uses lowercase attributes, why `XboxAxisPosition.Min` is −32767 and not
`i16::MIN`, why the engine's dispatch loop has no `break`. Those citations are
the only answer to those questions, and this file is what they resolve against.

## Where the C# is

The tree used to sit in `legacy/` in this repo — 312 files, 42 MB, including a
7.6 MB third-party driver installer (`Xbox360Accessories_x64_1.2.exe`) checked
into git. Nothing built from it: no path dependency, no `build.rs` reference, no
`include!`. It was deleted from the working tree. It still exists, unchanged, in
**two places that are the same commit**:

| where | how to reach it |
|---|---|
| upstream, on GitHub | <https://github.com/djlastnight/KeyboardSplitterXbox> at `863dc2dfd699fbd32394e26f365c7f50425e8af9` (`master`, last pushed 2023-10-28) |
| this repo, as a tag | `legacy-csharp-final` — **the identical SHA**, the commit this fork was taken from |

Because it is one commit, one file path works everywhere:

```
git show legacy-csharp-final:KeyboardSplitter/Presets/Preset.cs
https://github.com/djlastnight/KeyboardSplitterXbox/blob/863dc2d/KeyboardSplitter/Presets/Preset.cs
```

## How to read a citation

**Every `.cs` path in a ksx doc comment is relative to that repository's root.**
`VirtualXbox/Enums/XboxButton.cs` means exactly the blob at the URL above. Paths
are *not* relative to anything in this repo, and the `legacy/` prefix some of
them used to carry has been removed precisely so that one convention holds
everywhere.

That the redirect is honest was checked, not assumed: all 311 deleted files (174
of them `.cs`, the rest projects, images and binaries) were compared by git blob
SHA against the upstream tree at that commit, and every one matched. The single
file under `legacy/` that was *ours* rather than upstream's was
`legacy/LEGACY.md` — this document, moved here rather than lost.

## The files worth opening

Behavior archaeology usually lands on one of these:

- `KeyboardSplitter/Models/Splitter.cs` — the translation pipeline: one keyboard
  fanning out to many slots, all-keys-up and opposite-axis rules, state diffing.
  Ported in `ksx-core/src/engine.rs`.
- `KeyboardSplitter/Presets/Preset.cs` — cross-category custom-function
  aggregation, `FilterByKey`/`GetKeys`.
- `KeyboardSplitter/Presets/PresetUpgrader.cs` — the v1→v2 preset schema
  upgrade, which `ksx-legacy-import` applies transparently instead of rewriting
  the document.
- `VirtualXbox/Enums/*.cs` — the bit-exact ID tables. These are **contractual**:
  legacy preset XML on disk stores the numbers, so `ksx-core/src/pad.rs` has to
  reproduce them exactly.
- `Interceptor/Interception.cs`, `Interceptor/KeysHelper.cs` — the capture loop,
  suppress semantics, E0/E1 scancodes and the corrected-key rule.
- `KeyboardSplitter/Managers/InputManager.cs` — emergency hotkeys and focus
  passthrough, plus the synchronous-UI-dispatch defect the rewrite exists to
  kill.

## What it is not

- **Not a dependency.** It never built here (VS2013-era toolchain, .NET
  Framework 4.0, PlatformToolset v120) and CI never touched it.
- **Not the importer's fixtures.** `ksx-legacy-import`'s golden corpus
  (`crates/ksx-legacy-import/tests/fixtures/*.xml`) came off the real cabinet
  install, not out of this tree — no XML file ever lived in `legacy/`. The
  importer stays pinned without it.
- **Not shippable.** Upstream ships no license file at all (`docs/DRIVERS.md`),
  and the tree carries embedded binaries — `devcon.exe`, ScpVBus, prebuilt DLLs.
  Nothing from it goes into a release artifact.

The working binary Victor actually runs is a separate thing again — a built copy
at `C:\Users\Victor\KeyboardSplitter\KeyboardSplitter.exe`, the production
fallback until ksx passes the M4–M6 cabinet gates.

## A note on `docs/research/`

`docs/research/design-architecture.md` §6 plans the `git mv` **into** `legacy/`,
and its repo-tree diagram shows the folder. Those are dated records of what was
decided in early 2026 and are deliberately left as written — a research file
edited to match a later decision stops being evidence of anything. Read them as
history; read this file for where the code is today.
