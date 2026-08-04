# Legacy reference implementation (frozen)

This is djlastnight's original KeyboardSplitterXbox C# solution, exactly as forked
(tag: `legacy-csharp-final`). It is **unmaintained reference material**:

- Toolchain: VS2013-era (.NET Framework 4.0, PlatformToolset v120). Not expected to
  build here, and never built by CI.
- Purpose: behavior archaeology for the Rust rewrite (`crates/`) and the source of
  the importer's golden-test semantics.
- The working binary Victor actually runs lives outside the repo
  (`C:\Users\Victor\KeyboardSplitter\KeyboardSplitter.exe`) and stays the production
  fallback until ksx passes the M4–M6 cabinet gates.

Key behavior files (referenced from `docs/research/`):

- `KeyboardSplitter/Models/Splitter.cs` — translation pipeline, one-kbd→many-slots
  fan-out, all-keys-up + opposite-axis rules, state diffing
- `KeyboardSplitter/Presets/Preset.cs` — cross-category custom-function aggregation
- `Interceptor/Interception.cs` — capture loop, suppress semantics, E0/E1 scancodes
- `KeyboardSplitter/Managers/InputManager.cs` — emergency hotkeys, focus passthrough
  (and the synchronous-UI-dispatch defect the rewrite exists to kill)
- `VirtualXbox/Enums/*.cs` — bit-exact ID tables the importer must honor

Do not ship anything from this tree in release artifacts (embedded `devcon.exe`,
ScpVBus binaries, prebuilt DLLs).
