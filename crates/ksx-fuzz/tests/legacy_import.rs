//! Fuzz surface 1 (PLAYBOOK "M3/M6 fuzzing"): the legacy UTF-16 XML importer.
//!
//! Entry points: `decode_xml` (arbitrary file bytes → text) and the three
//! parsers behind `import_dir`. Invariant: never panic on arbitrary bytes;
//! everything unmappable becomes a typed [`ksx_legacy_import::Warning`].
//! Seeds: the real cabinet corpus committed in
//! `crates/ksx-legacy-import/tests/fixtures/`.

use ksx_fuzz::mutated_bytes;
use ksx_legacy_import::{
    decode_xml, parse_games, parse_presets, parse_settings, sanitize_file_stem, GAMES_XML,
    PRESETS_XML, SETTINGS_XML,
};
use proptest::prelude::*;

const FIXTURES: &[&[u8]] = &[
    include_bytes!("../../ksx-legacy-import/tests/fixtures/splitter_presets.xml") as &[u8],
    include_bytes!("../../ksx-legacy-import/tests/fixtures/splitter_presets.original.bak")
        as &[u8],
    include_bytes!("../../ksx-legacy-import/tests/fixtures/splitter_games.xml") as &[u8],
    include_bytes!("../../ksx-legacy-import/tests/fixtures/splitter_settings.xml") as &[u8],
];

fn seeds() -> Vec<Vec<u8>> {
    FIXTURES.iter().map(|f| f.to_vec()).collect()
}

proptest! {
    #![proptest_config(ksx_fuzz::persisting("regressions-legacy-import.txt"))]

    /// Arbitrary bytes → decode → all three parsers. Every parser sees every
    /// input (a user can hand any file any name), and whatever parses must
    /// also render back to TOML — `LegacyImport::rendered_files` relies on it.
    #[test]
    fn importer_never_panics_on_arbitrary_bytes(bytes in mutated_bytes(seeds(), 4096)) {
        let (text, _replaced) = decode_xml(&bytes);

        let presets = parse_presets(&text, PRESETS_XML);
        let games = parse_games(&text, GAMES_XML);
        let settings = parse_settings(&text, SETTINGS_XML);

        // Failures are typed warnings that must render, never aborts.
        for warning in presets
            .warnings
            .iter()
            .chain(&games.warnings)
            .chain(&settings.warnings)
        {
            let rendered = format!("{warning:?}");
            prop_assert!(!rendered.is_empty());
        }

        for preset in &presets.presets {
            prop_assert!(
                toml::to_string(preset).is_ok(),
                "parsed preset '{}' failed to render as TOML",
                preset.name
            );
            // The importer derives the output filename from the parsed name;
            // that derivation must be total too.
            let _ = sanitize_file_stem(&preset.name);
        }
        prop_assert!(toml::to_string(&games.games).is_ok());
        let config = ksx_config::ConfigFile {
            settings: settings.settings,
            ..Default::default()
        };
        prop_assert!(toml::to_string(&config).is_ok());
    }
}
