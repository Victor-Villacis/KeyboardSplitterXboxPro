//! SOCD cleaning — *Simultaneous Opposing Cardinal Directions*
//! (docs/INPUT-TRANSFORMS.md §2.6).
//!
//! A stick can only be left OR right. A panel can hold both at once, and what
//! the pad should then report is a policy, not a fact — tournaments legislate
//! it (Capcom-style rules regulate simultaneous opposing input; *neutral* and
//! *up-priority* are the compliant answers).
//!
//! **ksx does not implement this as a new engine rule.** Victor's insight is
//! that chord-with-consumption (§1b) *is* the SOCD mechanism, and it already
//! shipped:
//!
//! - `[Left+Right] → `[`Binding::Consume`] — both keys suppressed, nothing
//!   emitted in their place, so the axis snaps to centre and the dpad bits
//!   clear. That is **neutral**, for free.
//! - `[Down+Up] → whatever UP drove` — both keys suppressed and the UP key's
//!   own output re-emitted by the chord, so Down is swallowed and Up survives.
//!   That is **up-priority**, the fighting-game standard, and it is one
//!   ordinary chord.
//!
//! So all this module does is GENERATE those chords from a per-slot
//! [`Socd`] setting. Everything downstream — consumption, the all-keys-up
//! rule, the one-batch release discipline, the zero-allocation hot path —
//! is the chord engine, unchanged.
//!
//! **Not implemented: last-wins / "snap tap".** It needs to know which
//! direction was pressed *most recently*, which is input history; the engine
//! is deliberately a pure function of the current key SET. It waits for the
//! transform stage (§3), not for a new binding shape.

use std::fmt;
use std::str::FromStr;

use crate::key::Key;
use crate::pad::{Axis, DpadDirection};
use crate::preset::{Binding, Chord, Preset};

/// What a slot does when both halves of an opposing pair are held.
///
/// [`Socd::Off`] is the default and generates nothing at all, so every preset
/// and every replay that predates SOCD behaves byte-for-byte as it did.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Socd {
    /// Report both, exactly as the keys say (today's behavior).
    #[default]
    Off,
    /// Opposing pairs cancel: Left+Right → centre, Up+Down → centre.
    Neutral,
    /// Left+Right → centre, but Up beats Down (the fighting-game standard:
    /// it makes down-back → up-back a jump instead of a crouch).
    UpPriority,
}

impl Socd {
    pub const ALL: &'static [Socd] = &[Socd::Off, Socd::Neutral, Socd::UpPriority];

    /// Canonical name — what a config file stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            Socd::Off => "off",
            Socd::Neutral => "neutral",
            Socd::UpPriority => "up-priority",
        }
    }

    /// Does this policy generate anything? `false` is the zero-change path.
    pub const fn is_active(self) -> bool {
        !matches!(self, Socd::Off)
    }
}

impl fmt::Display for Socd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown socd policy '{0}' (expected one of: off, neutral, up-priority)")]
pub struct UnknownSocd(pub String);

impl FromStr for Socd {
    type Err = UnknownSocd;

    /// Case- and separator-insensitive, like [`crate::Persona`]: config files
    /// are hand-edited, so `up_priority` and `UpPriority` must work too.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect();
        match normalized.as_str() {
            "off" | "none" | "raw" => Ok(Socd::Off),
            "neutral" | "cancel" | "nullify" => Ok(Socd::Neutral),
            "uppriority" | "up" | "upwins" => Ok(Socd::UpPriority),
            _ => Err(UnknownSocd(s.to_owned())),
        }
    }
}

/// Two keys of one preset that can drive OPPOSITE directions of the same
/// control at the same time — the thing an SOCD policy is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpposingPair {
    /// The two keys, in preset-entry order (stable, so generation is).
    pub keys: (Key, Key),
    /// Vertical pairs are the only ones [`Socd::UpPriority`] treats specially.
    pub vertical: bool,
    /// For a vertical pair, which of [`OpposingPair::keys`] is UP. `None` when
    /// the preset is ambiguous (one key drives both polarities), in which case
    /// even up-priority falls back to neutral rather than guessing.
    pub up: Option<Key>,
}

impl OpposingPair {
    /// The keys as a set, for comparison against a chord's constituents.
    fn matches(&self, a: Key, b: Key) -> bool {
        (self.keys.0 == a && self.keys.1 == b) || (self.keys.0 == b && self.keys.1 == a)
    }
}

/// Which CONTROL a directional binding drives.
///
/// A pad has three ways to say "right", and a game reads whichever one it was
/// written for. Buttons and triggers have no mechanism — they are read the same
/// way whatever the preset does — so they never produce a mismatch.
///
/// This is the one definition in ksx: `ksx_config::validate` re-exports it for
/// `Issue::MacroHoldsOtherMechanism`, [`crate::diagonal`] buckets by it, and the
/// two ksx-studio mirrors are pinned against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DirMechanism {
    Dpad,
    LeftStick,
    RightStick,
}

impl DirMechanism {
    /// Canonical order — dpad, then left stick, then right stick. What a
    /// coalesced diagonal lists its mechanisms in.
    pub const ALL: &'static [DirMechanism] = &[
        DirMechanism::Dpad,
        DirMechanism::LeftStick,
        DirMechanism::RightStick,
    ];

    /// The mechanism this binding drives, if it points anywhere at all.
    pub fn of(binding: Binding) -> Option<Self> {
        pointing(binding).map(|p| p.mechanism)
    }

    /// How a report names it. Wording is contractual: it appears inside
    /// `Issue::MacroHoldsOtherMechanism`'s message and on the mapper page.
    pub const fn describe(self) -> &'static str {
        match self {
            DirMechanism::Dpad => "the dpad",
            DirMechanism::LeftStick => "the left stick (lx/ly)",
            DirMechanism::RightStick => "the right stick (rx/ry)",
        }
    }
}

/// WHERE one binding points: which mechanism, which axis of it, which way.
///
/// "Positive" is right for a horizontal control and UP for a vertical one —
/// which is why `Axis::Y` reads its sign directly (legacy binds Up to
/// `AXIS_MAX` and Down to `AXIS_MIN`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pointing {
    pub mechanism: DirMechanism,
    /// `DPAD_V` | `AXIS_Y` | `AXIS_RY`.
    pub vertical: bool,
    /// Right for a horizontal control, UP for a vertical one.
    pub positive: bool,
}

/// Where `binding` points, or `None` when it points nowhere.
///
/// **The one function.** `opposes` is built on it, and so is
/// [`crate::diagonal::fold`] — which is the whole reason a diagonal can never
/// disagree with SOCD or with [`crate::Interrupt::Opposing`]. A CENTRED AXIS IS
/// NEVER A DIRECTION (`ksx_config::validate` has the same rule), so it is never
/// half of a diagonal either.
pub const fn pointing(binding: Binding) -> Option<Pointing> {
    const fn p(mechanism: DirMechanism, vertical: bool, positive: bool) -> Option<Pointing> {
        Some(Pointing {
            mechanism,
            vertical,
            positive,
        })
    }
    match binding {
        Binding::Dpad(DpadDirection::Left) => p(DirMechanism::Dpad, false, false),
        Binding::Dpad(DpadDirection::Right) => p(DirMechanism::Dpad, false, true),
        Binding::Dpad(DpadDirection::Down) => p(DirMechanism::Dpad, true, false),
        Binding::Dpad(DpadDirection::Up) => p(DirMechanism::Dpad, true, true),
        Binding::Axis { value: 0, .. } => None, // centre drives no direction
        Binding::Axis { axis, value } => match axis {
            Axis::X => p(DirMechanism::LeftStick, false, value > 0),
            Axis::Y => p(DirMechanism::LeftStick, true, value > 0),
            Axis::Rx => p(DirMechanism::RightStick, false, value > 0),
            Axis::Ry => p(DirMechanism::RightStick, true, value > 0),
        },
        Binding::Button(_) | Binding::Trigger(_) | Binding::Consume => None,
    }
}

/// Control ids. Horizontal and vertical halves are separate controls, because
/// SOCD treats them differently and a key can be bound to both. Derived from
/// [`Pointing`] so there is still only one relation.
const DPAD_H: u8 = 0;
const DPAD_V: u8 = 1;
const AXIS_X: u8 = 2;
const AXIS_Y: u8 = 3;
const AXIS_RX: u8 = 4;
const AXIS_RY: u8 = 5;

const fn is_vertical(control: u8) -> bool {
    matches!(control, DPAD_V | AXIS_Y | AXIS_RY)
}

const fn control_of(p: Pointing) -> u8 {
    match (p.mechanism, p.vertical) {
        (DirMechanism::Dpad, false) => DPAD_H,
        (DirMechanism::Dpad, true) => DPAD_V,
        (DirMechanism::LeftStick, false) => AXIS_X,
        (DirMechanism::LeftStick, true) => AXIS_Y,
        (DirMechanism::RightStick, false) => AXIS_RX,
        (DirMechanism::RightStick, true) => AXIS_RY,
    }
}

/// `(control, positive)` for a binding that points somewhere, else `None`.
fn direction(binding: Binding) -> Option<(u8, bool)> {
    pointing(binding).map(|p| (control_of(p), p.positive))
}

/// Do these two bindings point OPPOSITE ways along the same control?
///
/// The one definition of "opposing" in ksx: SOCD cleaning cancels such a pair
/// (§2.6), and [`crate::Interrupt::Opposing`] aborts a macro when the player
/// presses one against a direction the macro is holding (§1c). Two features,
/// one relation — a diagonal that counts as opposing in one place must count in
/// the other, or the cabinet behaves differently depending on which feature you
/// read about.
///
/// Non-directional bindings (buttons, triggers, a centred axis, `Consume`)
/// oppose nothing.
pub fn opposes(a: Binding, b: Binding) -> bool {
    match (pointing(a), pointing(b)) {
        (Some(x), Some(y)) => {
            x.mechanism == y.mechanism && x.vertical == y.vertical && x.positive != y.positive
        }
        _ => false,
    }
}

/// One key's directional footprint in a preset.
struct KeyDirections {
    key: Key,
    /// Every `(control, positive)` this key drives.
    dirs: Vec<(u8, bool)>,
    /// Everything this key drives, directional or not — what up-priority has
    /// to re-emit when it consumes the key's partner.
    bindings: Vec<Binding>,
}

impl KeyDirections {
    fn drives(&self, control: u8, positive: bool) -> bool {
        self.dirs.contains(&(control, positive))
    }
}

fn key_directions(preset: &Preset) -> Vec<KeyDirections> {
    let mut out: Vec<KeyDirections> = Vec::new();
    for &(key, binding) in &preset.entries {
        if key == Key::None || binding == Binding::Consume {
            continue;
        }
        let slot = match out.iter().position(|k| k.key == key) {
            Some(i) => &mut out[i],
            None => {
                out.push(KeyDirections {
                    key,
                    dirs: Vec::new(),
                    bindings: Vec::new(),
                });
                out.last_mut().expect("just pushed")
            }
        };
        if !slot.bindings.contains(&binding) {
            slot.bindings.push(binding);
        }
        if let Some(dir) = direction(binding) {
            if !slot.dirs.contains(&dir) {
                slot.dirs.push(dir);
            }
        }
    }
    out
}

/// Every key pair this preset can hold in simultaneous opposition, in preset
/// order.
///
/// **Pairwise, on purpose.** Multi-bind is native in ksx (several keys per
/// direction), and the honest way to cover it is one chord per key pair that
/// can actually produce the opposition — `n × m` for `n` left keys and `m`
/// right keys, which on a real panel is 1×1 and on a pathological one is still
/// a handful. Evaluating the policy "at the direction level" instead would
/// need a rule the engine does not have; pairwise stays inside the chord
/// primitive that already shipped.
///
/// A pair that opposes on BOTH axes (a key bound to Left *and* Up against one
/// bound to Right *and* Down) is reported as horizontal: neutral is the
/// stronger rule, and this beats guessing.
pub fn opposing_pairs(preset: &Preset) -> Vec<OpposingPair> {
    let keys = key_directions(preset);
    let mut pairs = Vec::new();
    for (i, a) in keys.iter().enumerate() {
        for b in keys.iter().skip(i + 1) {
            let mut horizontal = false;
            let mut vertical: Option<Option<Key>> = None;
            for &(control, positive) in &a.dirs {
                if !b.drives(control, !positive) {
                    continue;
                }
                if is_vertical(control) {
                    // Which side is UP — unless a key drives both polarities,
                    // in which case there is no honest answer.
                    let up = match (
                        a.drives(control, true) && !a.drives(control, false),
                        b.drives(control, true) && !b.drives(control, false),
                    ) {
                        (true, false) => Some(a.key),
                        (false, true) => Some(b.key),
                        _ => None,
                    };
                    // First vertical control wins; a later ambiguous one must
                    // not un-decide it.
                    vertical = Some(vertical.flatten().or(up));
                } else {
                    horizontal = true;
                }
            }
            match (horizontal, vertical) {
                (true, _) => pairs.push(OpposingPair {
                    keys: (a.key, b.key),
                    vertical: false,
                    up: None,
                }),
                (false, Some(up)) => pairs.push(OpposingPair {
                    keys: (a.key, b.key),
                    vertical: true,
                    up,
                }),
                (false, None) => {}
            }
        }
    }
    pairs
}

/// A user-authored chord over exactly this pair, if there is one.
///
/// Generation never touches such a pair: **the user's chord wins**, because a
/// hand-written `[Left+Right] → something` is a deliberate statement and a
/// generated rule is a default. Validation reports the shadowing so it is
/// never silent.
pub fn shadowing_chord<'a>(preset: &'a Preset, pair: &OpposingPair) -> Option<&'a Chord> {
    preset.chords.iter().find(|chord| {
        chord.when.len() == 1 && pair.matches(chord.key, chord.when[0]) && chord.unless.is_empty()
    })
}

/// The chords `policy` adds to `preset`.
///
/// Idempotent: a chord the preset already carries (same trigger, guard and
/// output) is never duplicated, so applying twice is applying once.
pub fn generate(preset: &Preset, policy: Socd) -> Vec<Chord> {
    if !policy.is_active() {
        return Vec::new();
    }
    let keys = key_directions(preset);
    let bindings_of = |key: Key| -> Vec<Binding> {
        keys.iter()
            .find(|k| k.key == key)
            .map(|k| k.bindings.clone())
            .unwrap_or_default()
    };

    let mut out: Vec<Chord> = Vec::new();
    for pair in opposing_pairs(preset) {
        if shadowing_chord(preset, &pair).is_some() {
            continue;
        }
        let (a, b) = pair.keys;
        let mut fresh: Vec<Chord> = Vec::new();
        match (policy, pair.vertical, pair.up) {
            // Up-priority: consume BOTH and re-emit everything the UP key
            // drove. Consumption is all-or-nothing per chord, so "keep Up"
            // means "say Up again" — and it must be said in full, or the up
            // key's other bindings would vanish with it.
            (Socd::UpPriority, true, Some(up)) => {
                let down = if up == a { b } else { a };
                for binding in bindings_of(up) {
                    fresh.push(Chord::new(down, binding, vec![up]));
                }
            }
            // Neutral, and every horizontal pair under either policy.
            _ => fresh.push(Chord::consuming(a, vec![b])),
        }
        for chord in fresh {
            if !preset.chords.contains(&chord) && !out.contains(&chord) {
                out.push(chord);
            }
        }
    }
    out
}

impl Preset {
    /// Append the chords `policy` implies (see [`generate`]). A no-op for
    /// [`Socd::Off`], which is what makes the default byte-identical.
    pub fn apply_socd(&mut self, policy: Socd) {
        let generated = generate(self, policy);
        self.chords.extend(generated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad::{Trigger, XButton, AXIS_MAX, AXIS_MIN};

    fn stick_preset() -> Preset {
        Preset {
            name: "stick".into(),
            entries: vec![
                (
                    Key::Left,
                    Binding::Axis {
                        axis: Axis::X,
                        value: AXIS_MIN,
                    },
                ),
                (
                    Key::Right,
                    Binding::Axis {
                        axis: Axis::X,
                        value: AXIS_MAX,
                    },
                ),
                (
                    Key::Up,
                    Binding::Axis {
                        axis: Axis::Y,
                        value: AXIS_MAX,
                    },
                ),
                (
                    Key::Down,
                    Binding::Axis {
                        axis: Axis::Y,
                        value: AXIS_MIN,
                    },
                ),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        }
    }

    #[test]
    fn off_generates_nothing_at_all() {
        // The regression guarantee, stated as code.
        assert!(generate(&Preset::builtin_default(), Socd::Off).is_empty());
        let mut preset = Preset::builtin_default();
        preset.apply_socd(Socd::Off);
        assert_eq!(preset, Preset::builtin_default());
    }

    #[test]
    fn names_round_trip_and_aliases_parse() {
        for &policy in Socd::ALL {
            assert_eq!(policy.as_str().parse::<Socd>(), Ok(policy));
            assert_eq!(policy.to_string(), policy.as_str());
        }
        assert_eq!("up_priority".parse(), Ok(Socd::UpPriority));
        assert_eq!("UpPriority".parse(), Ok(Socd::UpPriority));
        assert_eq!(Socd::default(), Socd::Off);
        let err = "snaptap".parse::<Socd>().unwrap_err();
        assert!(err.to_string().contains("up-priority"), "{err}");
    }

    #[test]
    fn neutral_cancels_both_pairs() {
        let chords = generate(&stick_preset(), Socd::Neutral);
        assert_eq!(
            chords,
            vec![
                Chord::consuming(Key::Left, vec![Key::Right]),
                Chord::consuming(Key::Up, vec![Key::Down]),
            ]
        );
        assert!(chords.iter().all(|c| !c.emits()));
    }

    #[test]
    fn up_priority_keeps_up_and_still_neutralizes_horizontal() {
        let chords = generate(&stick_preset(), Socd::UpPriority);
        assert_eq!(
            chords,
            vec![
                Chord::consuming(Key::Left, vec![Key::Right]),
                // Down is the trigger; the chord re-says what Up drove.
                Chord::new(
                    Key::Down,
                    Binding::Axis {
                        axis: Axis::Y,
                        value: AXIS_MAX
                    },
                    vec![Key::Up]
                ),
            ]
        );
    }

    #[test]
    fn the_dpad_is_covered_too_and_so_is_the_right_stick() {
        let preset = Preset {
            name: "d".into(),
            entries: vec![
                (Key::J, Binding::Dpad(DpadDirection::Left)),
                (Key::L, Binding::Dpad(DpadDirection::Right)),
                (Key::I, Binding::Dpad(DpadDirection::Up)),
                (Key::K, Binding::Dpad(DpadDirection::Down)),
                (
                    Key::Numpad2,
                    Binding::Axis {
                        axis: Axis::Ry,
                        value: AXIS_MIN,
                    },
                ),
                (
                    Key::Numpad8,
                    Binding::Axis {
                        axis: Axis::Ry,
                        value: AXIS_MAX,
                    },
                ),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        };
        let chords = generate(&preset, Socd::UpPriority);
        assert_eq!(
            chords,
            vec![
                Chord::consuming(Key::J, vec![Key::L]),
                Chord::new(Key::K, Binding::Dpad(DpadDirection::Up), vec![Key::I]),
                Chord::new(
                    Key::Numpad2,
                    Binding::Axis {
                        axis: Axis::Ry,
                        value: AXIS_MAX
                    },
                    vec![Key::Numpad8]
                ),
            ]
        );
    }

    #[test]
    fn multi_bind_generates_every_pair_that_can_oppose() {
        // Two keys per direction: four ways to hold Left and Right at once.
        let axis = |value| Binding::Axis {
            axis: Axis::X,
            value,
        };
        let preset = Preset {
            name: "multi".into(),
            entries: vec![
                (Key::A, axis(AXIS_MIN)),
                (Key::S, axis(-16384)),
                (Key::D, axis(AXIS_MAX)),
                (Key::F, axis(16384)),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        };
        let chords = generate(&preset, Socd::Neutral);
        assert_eq!(chords.len(), 4);
        for (trigger, when) in [
            (Key::A, Key::D),
            (Key::A, Key::F),
            (Key::S, Key::D),
            (Key::S, Key::F),
        ] {
            assert!(
                chords.contains(&Chord::consuming(trigger, vec![when])),
                "{trigger:?}+{when:?} missing from {chords:?}"
            );
        }
    }

    #[test]
    fn up_priority_re_emits_every_binding_the_up_key_drove() {
        // Up drives the stick AND the dpad: consuming it must give back both.
        let preset = Preset {
            name: "both".into(),
            entries: vec![
                (
                    Key::Up,
                    Binding::Axis {
                        axis: Axis::Y,
                        value: AXIS_MAX,
                    },
                ),
                (Key::Up, Binding::Dpad(DpadDirection::Up)),
                (
                    Key::Down,
                    Binding::Axis {
                        axis: Axis::Y,
                        value: AXIS_MIN,
                    },
                ),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        };
        let chords = generate(&preset, Socd::UpPriority);
        assert_eq!(chords.len(), 2);
        assert!(chords
            .iter()
            .all(|c| c.key == Key::Down && c.when == [Key::Up]));
        assert!(chords.iter().any(|c| c.binding
            == Binding::Axis {
                axis: Axis::Y,
                value: AXIS_MAX
            }));
        assert!(chords
            .iter()
            .any(|c| c.binding == Binding::Dpad(DpadDirection::Up)));
    }

    #[test]
    fn non_directional_and_centred_bindings_are_never_paired() {
        let preset = Preset {
            name: "flat".into(),
            entries: vec![
                (Key::S, Binding::Button(XButton::A)),
                (Key::D, Binding::Button(XButton::B)),
                (Key::Q, Binding::Trigger(Trigger::Left)),
                (
                    Key::W,
                    Binding::Axis {
                        axis: Axis::X,
                        value: 0,
                    },
                ),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        };
        assert!(opposing_pairs(&preset).is_empty());
        assert!(generate(&preset, Socd::UpPriority).is_empty());
    }

    #[test]
    fn a_user_chord_over_the_pair_wins_and_is_reported() {
        let mut preset = stick_preset();
        preset.chords.push(Chord::new(
            Key::Right,
            Binding::Trigger(Trigger::Right),
            vec![Key::Left],
        ));
        let pairs = opposing_pairs(&preset);
        let horizontal = pairs.iter().find(|p| !p.vertical).expect("L+R pair");
        // Order-insensitive: the user wrote it the other way round.
        assert!(shadowing_chord(&preset, horizontal).is_some());
        // ...so nothing is generated for it, and the vertical pair still is.
        let chords = generate(&preset, Socd::Neutral);
        assert_eq!(chords, vec![Chord::consuming(Key::Up, vec![Key::Down])]);
    }

    #[test]
    fn generation_is_idempotent() {
        let mut preset = stick_preset();
        preset.apply_socd(Socd::UpPriority);
        let once = preset.chords.clone();
        preset.apply_socd(Socd::UpPriority);
        assert_eq!(preset.chords, once);
    }

    /// `opposes` and `pointing` are ONE relation, and this is what says so.
    /// A diagonal that counts as opposing in one place must count in the other:
    /// `crate::diagonal::fold` refuses to fold exactly the buckets `opposes`
    /// calls opposing, because both read the same function.
    #[test]
    fn opposition_is_pointing_and_nothing_else() {
        let vocabulary = [
            Binding::Dpad(DpadDirection::Up),
            Binding::Dpad(DpadDirection::Down),
            Binding::Dpad(DpadDirection::Left),
            Binding::Dpad(DpadDirection::Right),
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN,
            },
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MAX,
            },
            Binding::Axis {
                axis: Axis::X,
                value: -16384,
            },
            Binding::Axis {
                axis: Axis::Y,
                value: AXIS_MIN,
            },
            Binding::Axis {
                axis: Axis::Y,
                value: AXIS_MAX,
            },
            Binding::Axis {
                axis: Axis::Ry,
                value: AXIS_MAX,
            },
            Binding::Axis {
                axis: Axis::Rx,
                value: AXIS_MIN,
            },
            Binding::Axis {
                axis: Axis::X,
                value: 0,
            },
            Binding::Button(XButton::A),
            Binding::Trigger(Trigger::Left),
            Binding::Consume,
        ];
        for a in vocabulary {
            // A centred axis points nowhere — the rule diagonals inherit.
            assert_eq!(
                pointing(a).is_none(),
                matches!(
                    a,
                    Binding::Button(_)
                        | Binding::Trigger(_)
                        | Binding::Consume
                        | Binding::Axis { value: 0, .. }
                ),
                "{a:?}"
            );
            for b in vocabulary {
                let by_pointing = match (pointing(a), pointing(b)) {
                    (Some(x), Some(y)) => {
                        x.mechanism == y.mechanism
                            && x.vertical == y.vertical
                            && x.positive != y.positive
                    }
                    _ => false,
                };
                assert_eq!(opposes(a, b), by_pointing, "{a:?} vs {b:?}");
                assert_eq!(opposes(a, b), opposes(b, a), "asymmetric: {a:?} {b:?}");
            }
        }
        // The mechanism a report names, and its exact wording.
        assert_eq!(
            DirMechanism::of(Binding::Dpad(DpadDirection::Up)),
            Some(DirMechanism::Dpad)
        );
        assert_eq!(
            DirMechanism::of(Binding::Axis {
                axis: Axis::Ry,
                value: AXIS_MAX
            }),
            Some(DirMechanism::RightStick)
        );
        assert_eq!(
            DirMechanism::of(Binding::Axis {
                axis: Axis::X,
                value: 0
            }),
            None
        );
        assert_eq!(DirMechanism::Dpad.describe(), "the dpad");
        assert_eq!(DirMechanism::LeftStick.describe(), "the left stick (lx/ly)");
        assert_eq!(
            DirMechanism::RightStick.describe(),
            "the right stick (rx/ry)"
        );
    }

    #[test]
    fn a_key_bound_to_both_polarities_falls_back_to_neutral() {
        // Both keys drive Up AND Down; there is no honest "up key", so
        // up-priority must not guess — it neutralizes instead.
        let preset = Preset {
            name: "ambiguous".into(),
            entries: vec![
                (Key::W, Binding::Dpad(DpadDirection::Up)),
                (Key::W, Binding::Dpad(DpadDirection::Down)),
                (Key::S, Binding::Dpad(DpadDirection::Up)),
                (Key::S, Binding::Dpad(DpadDirection::Down)),
            ],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        };
        let pairs = opposing_pairs(&preset);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].vertical && pairs[0].up.is_none());
        assert_eq!(
            generate(&preset, Socd::UpPriority),
            vec![Chord::consuming(Key::W, vec![Key::S])]
        );
    }
}
