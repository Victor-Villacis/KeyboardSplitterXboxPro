# Forma spike 1 — FMIR compat: today's TS toolchain vs forma-server 0.1.4 (run 2026-08-04)

Spike for `docs/ENHANCEMENTS.md` E7. Question: does FMIR produced by TODAY's
`@getforma` npm packages on Windows parse and render in the frozen (March)
`forma-server` 0.1.4? Full working tree in the session scratchpad
(`forma-spike/` — npm project + `fmir-probe` cargo bin).

## VERDICT: COMPATIBLE. Proceed — no pin, no wait.

The current compiler still emits **FMIR v2**, and 0.1.4 still expects **FMIR
v2**. Clean parse, `check_ir_compatibility` passes, `render_page` produces
correct Phase-2 SSR HTML — for both a static page and a dynamic page with
signal slots. Step 3 (downgrade fallback) was never needed. The 5-month
npm-side version drift (core 1.x, compiler 0.1.8) happened entirely *above*
the binary format; the IR contract itself has not moved.

## Versions tested

| Side | Package | Version |
|---|---|---|
| TS | `@getforma/core` | **1.5.0** |
| TS | `@getforma/compiler` | **0.1.8** (emits the FMIR) |
| TS | `@getforma/build` | **0.1.8** |
| Rust | `forma-server` | **0.1.4** (latest on crates.io as of today) |
| Rust | `forma-ir` | **0.1.4** (pulled by forma-server; latest on crates.io) |
| Toolchain | Node 20.16.0 / npm 10.8.1 · rustc + cargo 1.96.0 stable MSVC · Windows 11 Pro |

## Evidence

**Both sides hardcode the same format version.** Compiler
(`@getforma/compiler/dist/index.js`, `IrEmitContext.toBinary()`) writes magic
`FMIR` + u16 LE `2` at offset 4, 16-byte header + 32-byte section table
(data starts at 48). Rust (`forma-ir-0.1.4/src/format.rs`):

```rust
pub const MAGIC: &[u8; 4] = b"FMIR";
pub const HEADER_SIZE: usize = 16;      // + SECTION_TABLE_SIZE = 32
pub const IR_VERSION: u16 = 2;
```

`forma-server::check_ir_compatibility` is exactly
`module.header.version != IR_VERSION → Err`, nothing more.

**Build (TS side).** Minimal `mount(() => HomePage(), "#app")` app with an
`h()` component, built via `@getforma/build` with `ssr: true`:

```
IR emitted (real): app.ir (183 bytes, 0 islands)      // static page
IR emitted (real): dyn-app.ir (224 bytes, 0 islands)  // 2 createSignal slots
```

Hex of `app.*.ir` starts `46 4d 49 52 02 00` — `FMIR`, version 2.

**Probe (Rust side).** `fmir-probe` feeds the .ir + the real `manifest.json`
(which deserializes into `forma_server::AssetManifest` unchanged) through
`IrModule::parse` → `check_ir_compatibility` → `render_page` with
`RenderMode::Phase2SsrReconcile` and `SlotData::new_from_defaults`:

```
PARSE: OK  (header.version=2, flags=0, strings=7, slots=0, islands=0, opcode_bytes=54)
check_ir_compatibility: OK (compiler v2 == runtime v2)
<div id="app" data-forma-ssr><div class="page"><h1>ksx forma spike</h1>
<p>Hello from the Forma compiler</p></div></div>
VERDICT: Phase 2 SSR succeeded — IR walked to real HTML
```

Dynamic page: `PARSE: OK (strings=10, slots=2, opcode_bytes=79)`, compat OK,
and the walker rendered signal defaults with hydration markers:

```
<p id="status"><!--f:t0-->idle<!--/f:t0--></p><span><!--f:t1-->0<!--/f:t1--></span>
```

No panic, no garbage, correct CSP header emitted alongside.

## Windows friction (spike 2's question, answered along the way)

Near-zero for the path ksx needs:

- `npm install` clean — no native compile step; esbuild's prebuilt
  `@esbuild/win32-x64` binary lands via optionalDependencies.
- No `tsx` needed despite the README's `npx tsx build.ts`: write the build
  script as `build.mjs` and run `node build.mjs`. Entry points can stay `.ts`
  (esbuild inside the pipeline handles them); the compiler also transforms
  `.tsx` via `esbuild.transformSync` for IR analysis.
- Path handling all correct: relative-import resolution in `generateRealIr`
  (readFileSync + dirname), hashed output, manifest keys, backslash console
  output — no cygpath-style games needed anywhere.
- **One real, latent bug — Tailwind CSS entries are broken on Windows.**
  `@getforma/build` runs `execFileSync("npx", ["@tailwindcss/cli", ...])`
  without `shell: true`; on Windows `npx` is `npx.cmd`, so this throws ENOENT
  (verified directly: `execFileSync('npx',['--version'])` → ENOENT). Not hit
  in this spike (no `cssEntries`); avoid `{ tailwind: true }` entries or fix
  upstream. Plain CSS concat entries use `readFileSync` and are fine.
- `wasm-pack` invocation is `execFileSync("wasm-pack", ...)` — a real .exe,
  so that path should be fine (not exercised; no wasm config).

## What this means for M10 Studio

- **Proceed** with current npm packages (`core@1.5.0`, `compiler@0.1.8`,
  `build@0.1.8`) against `forma-server = "0.1.4"` / `forma-ir = "0.1.4"`.
  Neither pinning old TS packages nor waiting for new Rust crates is needed.
- The compat surface is a single u16. If a future compiler bumps to FMIR v3,
  0.1.4 refuses loudly (`"IR version 3 is not compatible with runtime version
  2"`) and `load_ir_modules` degrades to Phase-1 client mount rather than
  crashing — so drift is detectable and non-fatal. Cheap insurance: keep the
  48-byte header hexdump check (`46 4d 49 52 02 00`) as a build-time assert
  in Studio's pipeline.
- `forma-server`'s manifest/PageConfig API consumed `@getforma/build`'s
  `manifest.json` byte-for-byte with serde — the two halves are still one
  stack in practice, not just in the README.
