//! Instantiating an in-box template into a preset file — the write half of
//! `ksx preset new`, extracted so it has exactly one implementation.
//!
//! [`crate::preset_cli`] is the console driver: it parses flags, prints TOML
//! for `--dry-run`, and exits with a code. Everything it *decides* is here,
//! in the same typed-spec / pure-plan / one-writer shape as
//! [`crate::device_edit`] — because the console is no longer the only caller.
//! Studio's Profiles page performs the identical verb, and a second copy of
//! "refuse to clobber, then back up, then save" is how the two grow apart:
//! one of them gets the backup, the other does not, and nobody finds out
//! until a preset is gone.
//!
//! The split is drawn where the process boundary is. [`plan_new`] takes the
//! path of the preset already on disk (or `None`) rather than a `Store`, so
//! every refusal is unit-testable with no config root; [`apply_new`] is the
//! only thing that touches the filesystem, and it takes the backup first.

use std::path::{Path, PathBuf};

use ksx_config::{ConfigError, PresetFile, Store};
use ksx_core::templates::{self, TemplateError};

/// One `ksx preset new`, as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPresetSpec {
    pub name: String,
    /// The template id (`ksx_core::templates::TEMPLATES`).
    pub template: String,
    /// Which player block, 1-based.
    pub player: u8,
    /// Overwrite an existing preset of that name (a backup is taken first).
    pub force: bool,
}

/// Everything `new` decided, before a byte is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPresetPlan {
    /// The file, whole, ready to serialize.
    pub file: PresetFile,
    /// The template it came from, as asked for.
    pub template: String,
    pub player: u8,
    /// A preset of this name is already on disk and `--force` was given.
    pub overwrites: bool,
}

/// Why a preset could not be written.
#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{0}")]
    Template(#[from] TemplateError),
    #[error("a preset called \"{name}\" already exists ({})", path.display())]
    Exists { name: String, path: PathBuf },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl PresetError {
    /// The stable refusal word — the same one `ksx preset new --json` prints,
    /// which is why the mapping lives here and not in the console driver.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Template(TemplateError::Unknown(_)) => "unknown-template",
            Self::Template(TemplateError::NoSuchPlayer { .. }) => "no-such-player",
            Self::Template(TemplateError::EmptyName) => "bad-name",
            Self::Exists { .. } => "preset-exists",
            Self::Config(_) => "config-error",
        }
    }

    /// What to do about it.
    pub fn advice(&self) -> Option<String> {
        match self {
            Self::Template(_) => {
                Some("`ksx preset list --templates` names the ones that ship.".to_owned())
            }
            Self::Exists { .. } => {
                Some("--force overwrites it (a timestamped backup is taken first).".to_owned())
            }
            Self::Config(_) => None,
        }
    }
}

/// Decide the whole file. Pure: no store, no disk.
///
/// `existing` is the path of the preset already on disk under this name, or
/// `None`. It is a path rather than a bool so the refusal can print WHICH file
/// it is protecting — a user with a portable install and an installed one has
/// two, and "already exists" without a path is a sentence about neither.
pub fn plan_new(
    existing: Option<&Path>,
    spec: &NewPresetSpec,
) -> Result<NewPresetPlan, PresetError> {
    let preset = templates::instantiate(&spec.template, &spec.name, spec.player)?;
    if let Some(path) = existing {
        if !spec.force {
            return Err(PresetError::Exists {
                name: spec.name.clone(),
                path: path.to_path_buf(),
            });
        }
    }
    Ok(NewPresetPlan {
        file: PresetFile::from_core(&preset),
        template: spec.template.clone(),
        player: spec.player,
        overwrites: existing.is_some(),
    })
}

/// What landed on disk.
#[derive(Clone, Debug)]
pub struct NewPresetOutcome {
    pub path: PathBuf,
    /// The timestamped copy taken before an overwrite; `None` for a new file.
    pub backup: Option<PathBuf>,
    pub plan: NewPresetPlan,
}

/// Write the file `plan` describes. Only ever destructive with `force`, and
/// never without a copy first.
pub fn apply_new(store: &Store, plan: &NewPresetPlan) -> Result<NewPresetOutcome, PresetError> {
    let backup = if plan.overwrites {
        store.backup(&store.preset_path(&plan.file.name)?)?
    } else {
        None
    };
    let written = store.save_preset(&plan.file)?;
    Ok(NewPresetOutcome {
        path: written,
        backup,
        plan: plan.clone(),
    })
}

impl NewPresetOutcome {
    /// The one-line report a surface flashes — one line, and under the
    /// 300-character cap `ksx-studio`'s flash truncates at.
    pub fn message(&self) -> String {
        format!(
            "{} preset \"{}\" — {} controls from \"{}\" (player {})",
            if self.plan.overwrites {
                "replaced"
            } else {
                "created"
            },
            self.plan.file.name,
            self.plan.file.bindings.len(),
            self.plan.template,
            self.plan.player,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, template: &str) -> NewPresetSpec {
        NewPresetSpec {
            name: name.to_owned(),
            template: template.to_owned(),
            player: 1,
            force: false,
        }
    }

    /// Task #14's template, which is the one a desk-keyboard owner needs and
    /// the reason the Profiles page offers a template list at all.
    #[test]
    fn the_two_player_desktop_keyboard_template_instantiates() {
        let plan = plan_new(None, &spec("Couch", "keyboard-2p")).unwrap();
        assert_eq!(plan.file.name, "Couch");
        assert!(!plan.file.bindings.is_empty());
        assert!(!plan.overwrites);
    }

    #[test]
    fn an_unknown_template_refuses_and_names_the_list() {
        let err = plan_new(None, &spec("X", "nope")).unwrap_err();
        assert_eq!(err.code(), "unknown-template");
        assert!(err.advice().unwrap().contains("--templates"));
    }

    #[test]
    fn a_player_block_the_template_does_not_carry_refuses() {
        let mut asked = spec("X", "keyboard-wasd");
        asked.player = 3;
        let err = plan_new(None, &asked).unwrap_err();
        assert_eq!(err.code(), "no-such-player");
    }

    /// The clobber guard, and the path in the sentence.
    #[test]
    fn an_existing_preset_is_never_overwritten_without_force() {
        let path = PathBuf::from("C:\\cfg\\presets\\Couch.toml");
        let err = plan_new(Some(&path), &spec("Couch", "keyboard-2p")).unwrap_err();
        assert_eq!(err.code(), "preset-exists");
        assert!(err.to_string().contains("Couch.toml"), "{err}");

        let mut forced = spec("Couch", "keyboard-2p");
        forced.force = true;
        let plan = plan_new(Some(&path), &forced).unwrap();
        assert!(plan.overwrites);
    }

    #[test]
    fn an_empty_name_refuses() {
        let err = plan_new(None, &spec("   ", "keyboard-2p")).unwrap_err();
        assert_eq!(err.code(), "bad-name");
    }
}
