//! ksx-fuzz — fuzz-style robustness tests for every parser that eats
//! untrusted bytes.
//!
//! `docs/PLAYBOOK.md` ("M3/M6 fuzzing") schedules `cargo-fuzz` targets for the
//! legacy-XML importer + TOML config (M3) and the raw NKRO HID report parser
//! (M6). This crate is the **deliberate fallback** for that plan on this host:
//!
//! # Why not cargo-fuzz here
//!
//! - This machine has exactly one toolchain: `stable-x86_64-pc-windows-msvc`
//!   (`rustup toolchain list`). `cargo fuzz` requires nightly for
//!   `-Zsanitizer`, and `cargo-fuzz` is not installed.
//! - libFuzzer + sanitizer support on `windows-msvc` is second-class; getting
//!   a working link is a toolchain fight the playbook's "local bursts, not
//!   24/7" posture does not justify.
//!
//! Instead: **proptest** with byte-vector strategies — `prop_oneof` over (a)
//! raw random bytes, (b) the real corpus seeds verbatim, and (c) seeds with
//! random byte edits / cuts / splices (the "structured-ish" half that keeps
//! coverage near the real formats). Case counts are env-tunable: default 256
//! keeps `cargo test` CI-fast; a burst is `PROPTEST_CASES=50000 cargo test -p
//! ksx-fuzz --release`. A Linux CI can later add true libFuzzer targets that
//! call the *same* entry points with the same seeds; nothing here blocks that.
//!
//! # Targets (one integration test per attack surface)
//!
//! - `tests/legacy_import.rs` — `ksx-legacy-import`: UTF-16 XML from untrusted
//!   files (`decode_xml` + `parse_presets`/`parse_games`/`parse_settings`),
//!   seeded from the real cabinet fixtures.
//! - `tests/config.rs` — `ksx-config`: TOML config/preset/games parsing,
//!   `Store` loads over arbitrary file bytes, `parse_function`,
//!   `preset_file_name`.
//! - `tests/hid_report.rs` — `ksx-capture::hid`: report-descriptor parsing and
//!   `ReportReader::feed` over hardware-supplied bytes (the M6 WinUSB path).
//!
//! # Invariants (never panic is the baseline)
//!
//! - Importer: parsing never panics on arbitrary bytes; failures are typed
//!   `Warning`s, never aborts.
//! - Config: never panics; failures are `toml::de::Error` / `ConfigError`.
//! - NKRO: `feed` never panics, never emits usage 0 or the error usages
//!   0x01–0x03, emits each usage at most once per report, and the number of
//!   press events is bounded by the report's bit count.

use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::FileFailurePersistence;

/// [`ProptestConfig`] that persists failing seeds to `regressions` (relative
/// to the crate root — cargo runs test binaries with cwd there). The default
/// `SourceParallel` persistence cannot find a `lib.rs`/`main.rs` anchor for
/// integration-test crates and silently drops the regression entry; every
/// `proptest!` block in `tests/` must use this instead so a found crash
/// leaves a committed repro behind.
pub fn persisting(regressions: &'static str) -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(regressions))),
        ..ProptestConfig::default()
    }
}

/// Byte-vector fuzz inputs: raw random bytes, a corpus seed verbatim, or a
/// seed with random point edits, one cut, and one splice.
///
/// The 2/1/4 weights keep most cases near the real formats (where the
/// interesting branches live) while still exercising pure noise.
pub fn mutated_bytes(seeds: Vec<Vec<u8>>, max_random: usize) -> BoxedStrategy<Vec<u8>> {
    let seed = proptest::sample::select(seeds);
    prop_oneof![
        2 => proptest::collection::vec(any::<u8>(), 0..max_random),
        1 => seed.clone(),
        4 => (
            seed,
            proptest::collection::vec((any::<Index>(), any::<u8>()), 0..48),
            proptest::option::of((any::<Index>(), 1usize..256)),
            proptest::option::of((any::<Index>(), proptest::collection::vec(any::<u8>(), 1..32))),
        )
            .prop_map(|(s, edits, cut, splice)| {
                let mut bytes = s;
                for (idx, byte) in edits {
                    if bytes.is_empty() {
                        break;
                    }
                    let i = idx.index(bytes.len());
                    bytes[i] = byte;
                }
                if let Some((at, len)) = cut {
                    if !bytes.is_empty() {
                        let start = at.index(bytes.len());
                        let end = (start + len).min(bytes.len());
                        bytes.drain(start..end);
                    }
                }
                if let Some((at, insert)) = splice {
                    let i = if bytes.is_empty() { 0 } else { at.index(bytes.len()) };
                    bytes.splice(i..i, insert);
                }
                bytes
            }),
    ]
    .boxed()
}

/// Text fuzz inputs for string-based parsers (TOML): [`mutated_bytes`] run
/// through lossy UTF-8, plus printable-ASCII noise (which is far more likely
/// to reach deep TOML branches than raw byte noise).
pub fn mutated_text(seeds: Vec<String>, max_random: usize) -> BoxedStrategy<String> {
    let byte_seeds: Vec<Vec<u8>> = seeds.into_iter().map(String::into_bytes).collect();
    prop_oneof![
        4 => mutated_bytes(byte_seeds, max_random)
            .prop_map(|b| String::from_utf8_lossy(&b).into_owned()),
        1 => "[ -~\\t\\r\\n]{0,512}",
        1 => ".{0,256}",
    ]
    .boxed()
}
