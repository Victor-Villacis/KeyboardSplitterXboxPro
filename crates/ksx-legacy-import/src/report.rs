//! Import outcome reporting: counts + precise per-entry warnings.

use std::fmt;

/// One precise warning about an unmappable/suspicious legacy construct.
/// Warnings never abort an import; the offending entry is skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// Legacy file name, e.g. `splitter_presets.xml`.
    pub file: String,
    /// Preset name, when the warning is scoped to one preset.
    pub preset: Option<String>,
    /// Offending entry rendered close to its XML form, e.g. `button id="99"`.
    pub entry: Option<String>,
    pub reason: String,
}

impl Warning {
    pub(crate) fn file_level(file: &str, reason: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            preset: None,
            entry: None,
            reason: reason.into(),
        }
    }

    pub(crate) fn in_preset(file: &str, preset: &str, reason: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            preset: Some(preset.to_owned()),
            entry: None,
            reason: reason.into(),
        }
    }

    pub(crate) fn entry(
        file: &str,
        preset: Option<&str>,
        entry: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            file: file.to_owned(),
            preset: preset.map(str::to_owned),
            entry: Some(entry.into()),
            reason: reason.into(),
        }
    }

    fn to_json(&self) -> String {
        let opt = |v: &Option<String>| match v {
            Some(s) => format!("\"{}\"", json_escape(s)),
            None => "null".to_owned(),
        };
        format!(
            "{{\"file\":\"{}\",\"preset\":{},\"entry\":{},\"reason\":\"{}\"}}",
            json_escape(&self.file),
            opt(&self.preset),
            opt(&self.entry),
            json_escape(&self.reason)
        )
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(preset) = &self.preset {
            write!(f, ": preset '{preset}'")?;
        }
        if let Some(entry) = &self.entry {
            write!(f, ": {entry}")?;
        }
        write!(f, ": {}", self.reason)
    }
}

/// Summary of one legacy import run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Number of presets imported.
    pub presets: usize,
    /// Number of games imported.
    pub games: usize,
    /// Expected legacy files that were not found (skipped, not an error).
    pub skipped: Vec<String>,
    pub warnings: Vec<Warning>,
}

impl ImportReport {
    /// Stable machine-readable form for `--json` automation.
    pub fn to_json(&self) -> String {
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .collect();
        let warnings: Vec<String> = self.warnings.iter().map(Warning::to_json).collect();
        format!(
            "{{\"presets\":{},\"games\":{},\"skipped\":[{}],\"warnings\":[{}]}}",
            self.presets,
            self.games,
            skipped.join(","),
            warnings.join(",")
        )
    }
}

impl fmt::Display for ImportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "presets imported: {}", self.presets)?;
        writeln!(f, "games imported:   {}", self.games)?;
        for skipped in &self.skipped {
            writeln!(f, "skipped (not found): {skipped}")?;
        }
        if self.warnings.is_empty() {
            write!(f, "warnings: none")?;
        } else {
            write!(f, "warnings: {}", self.warnings.len())?;
            for warning in &self.warnings {
                write!(f, "\n  {warning}")?;
            }
        }
        Ok(())
    }
}

/// Escape a string for embedding inside a JSON string literal.
///
/// Public so the CLI can compose its own JSON envelope around
/// [`ImportReport::to_json`] without a JSON dependency.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escaping_covers_specials() {
        assert_eq!(
            json_escape("C:\\a\"b\"\n\t\u{1}"),
            "C:\\\\a\\\"b\\\"\\n\\t\\u0001"
        );
    }

    #[test]
    fn report_json_is_well_formed() {
        let report = ImportReport {
            presets: 2,
            games: 1,
            skipped: vec!["splitter_settings.xml".into()],
            warnings: vec![Warning::entry(
                "splitter_presets.xml",
                Some("P1"),
                "button id=\"99\"",
                "unknown button id 99",
            )],
        };
        assert_eq!(
            report.to_json(),
            "{\"presets\":2,\"games\":1,\"skipped\":[\"splitter_settings.xml\"],\
             \"warnings\":[{\"file\":\"splitter_presets.xml\",\"preset\":\"P1\",\
             \"entry\":\"button id=\\\"99\\\"\",\"reason\":\"unknown button id 99\"}]}"
        );
    }

    #[test]
    fn display_forms_are_stable() {
        let warning = Warning::entry("f.xml", Some("P1"), "custom function=\"3\"", "bad");
        assert_eq!(
            warning.to_string(),
            "f.xml: preset 'P1': custom function=\"3\": bad"
        );
        let report = ImportReport {
            presets: 1,
            games: 0,
            skipped: vec![],
            warnings: vec![warning],
        };
        assert_eq!(
            report.to_string(),
            "presets imported: 1\ngames imported:   0\nwarnings: 1\n  f.xml: preset 'P1': custom function=\"3\": bad"
        );
    }
}
