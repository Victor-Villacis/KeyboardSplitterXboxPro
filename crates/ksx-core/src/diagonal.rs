//! Diagonals as PRESENTATION — the lens, not a new binding shape.
//!
//! Victor's insight, and the root cause of the evening it cost: *"if down and
//! right together equals diagonal, the user does not care — we can present the
//! diagonal in the piano and the user can select it, and behind the scenes we
//! do down and right, so it's seamless."*
//!
//! A diagonal IS two simultaneous holds. That is ksx's implementation detail —
//! it is not the user's concept. Fighting-game players think in ↘ /
//! down-forward / numpad 3, never in "two axis bindings held together", and no
//! mapper in the field lets them pick one: Steam Input's D-pad is four cardinal
//! binding slots, reWASD's own answer is "build a Shortcut out of two zones",
//! MAME's input map is four cardinals, GP2040-CE treats diagonals purely as
//! cardinal pairs. ksx's storage model is the same model the whole field uses;
//! what is missing is the lens.
//!
//! **So this module changes nothing that is stored.** A [`crate::MacroStep`]
//! still holds a set of ordinary [`Binding`]s, files stay hand-editable, the
//! engine is untouched, and old presets keep working. [`fold`] READS a hold and
//! reports how to PRESENT it; [`expand`] puts the strings back, byte for byte.
//!
//! Nothing in the engine calls anything here — `EngineTables::build` and the hot
//! path never see it, so `macros.rs`'s "nothing allocates after build" promise
//! is unaffected.
//!
//! # The one rule
//!
//! Per mechanism bucket, **"contains both"** — never exact-set-equality on the
//! whole step. `down + forward + A` is the most common macro step in existence
//! (the attack that ends a motion) and it must fold; exact-pair matching fails
//! it. `down + forward + up` is contradictory and must NEVER fold: which
//! diagonal would it be, and what the pad publishes depends on the slot's
//! `socd` policy resolved at plan time, which a reader of the step cannot see.

use crate::preset::Binding;
use crate::socd::{pointing, DirMechanism};

/// The four diagonals — ↖ ↗ ↙ ↘, numpad 7 9 1 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Diag {
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Diag {
    /// Canonical order. Numpad-ascending would put them in a different order
    /// than the ring the mapper draws; this is the order a `fold` emits.
    pub const ALL: &'static [Diag] =
        &[Diag::UpLeft, Diag::UpRight, Diag::DownLeft, Diag::DownRight];

    /// The glyph. Screens speak arrows in this genre — SF6's input history is
    /// arrow icons, every Capcom move list and arcade instruction card is
    /// arrows — so the arrow is what a mapper draws.
    pub const fn glyph(self) -> &'static str {
        match self {
            Diag::UpLeft => "↖",
            Diag::UpRight => "↗",
            Diag::DownLeft => "↙",
            Diag::DownRight => "↘",
        }
    }

    /// The numpad digit. A LOOKUP TOKEN, never the label: it is how the input
    /// is written in TEXT (SuperCombo/Dustloop call numpad "the modern standard
    /// for expressing inputs in text"), and it is already in a MAME cab owner's
    /// `mame.ini` — `-joystick_map` says the digits "use the same mapping that
    /// appears on the NumPad of a keyboard".
    pub const fn numpad(self) -> u8 {
        match self {
            Diag::UpLeft => 7,
            Diag::UpRight => 9,
            Diag::DownLeft => 1,
            Diag::DownRight => 3,
        }
    }

    /// The name, in words. COMPASS, not forward/back: ksx already offers the
    /// mirrored spelling of every motion because player 2 is not an edge case,
    /// and "down-forward" is only true for a character facing right. The
    /// forward/back spelling a move list uses is [`Diag::move_list`].
    pub const fn words(self) -> &'static str {
        match self {
            Diag::UpLeft => "up-left",
            Diag::UpRight => "up-right",
            Diag::DownLeft => "down-left",
            Diag::DownRight => "down-right",
        }
    }

    /// How a move list writes it — Tekken's official command lists spell
    /// `d/f`, literally "down-forward" as a hyphenated compound of two
    /// cardinals. Facing-relative, so it is offered BESIDE [`Diag::words`] and
    /// never instead of it.
    pub const fn move_list(self) -> &'static str {
        match self {
            Diag::UpLeft => "u/b",
            Diag::UpRight => "u/f",
            Diag::DownLeft => "d/b",
            Diag::DownRight => "d/f",
        }
    }

    /// `(up, right)` — the two polarities this diagonal is made of.
    pub const fn halves(self) -> (bool, bool) {
        match self {
            Diag::UpLeft => (true, false),
            Diag::UpRight => (true, true),
            Diag::DownLeft => (false, false),
            Diag::DownRight => (false, true),
        }
    }

    /// The diagonal made of those two polarities. Total: every `(up, right)`
    /// pair is a diagonal.
    pub const fn from_halves(up: bool, right: bool) -> Diag {
        match (up, right) {
            (true, false) => Diag::UpLeft,
            (true, true) => Diag::UpRight,
            (false, false) => Diag::DownLeft,
            (false, true) => Diag::DownRight,
        }
    }
}

/// One PRESENTED control: a diagonal, or anything that was not folded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    /// A recognized diagonal. `members` are INDICES into the original hold, in
    /// original order — the round trip is "put those strings back".
    Diagonal {
        diag: Diag,
        /// Every mechanism that reached this diagonal, in [`DirMechanism::ALL`]
        /// order. More than one is the hat+stick double-binding every in-box
        /// template writes.
        mechanisms: Vec<DirMechanism>,
        members: Vec<usize>,
        /// `false` when the members are not the canonical extremes (a
        /// hand-written `ly.-16384 + lx.max`). A LABEL flag, never a rewrite
        /// flag: the step is presented as the diagonal it is, and the exact
        /// values stay exactly as the file spells them.
        exact: bool,
    },
    /// Anything not folded: a leftover cardinal, a button, a trigger,
    /// `consume`, a centred axis, or an unparseable name.
    Plain { member: usize },
}

/// A parsed hold entry, or `None` for one that points nowhere.
fn slots(hold: &[Binding]) -> Vec<Option<(DirMechanism, bool, bool, bool)>> {
    hold.iter()
        .map(|&b| {
            pointing(b).map(|p| {
                let exact = match b {
                    Binding::Axis { value, .. } => {
                        value == crate::pad::AXIS_MIN || value == crate::pad::AXIS_MAX
                    }
                    _ => true,
                };
                (p.mechanism, p.vertical, p.positive, exact)
            })
        })
        .collect()
}

/// How to PRESENT this hold. See the module docs for the rule.
///
/// Emission order: diagonals first (in [`Diag::ALL`] order), then everything
/// else in original index order — so a step's readout leads with the shape and
/// ends with the button.
pub fn fold(hold: &[Binding]) -> Vec<Held> {
    let parsed = slots(hold);

    // Per mechanism: the distinct polarities seen on each axis, and which
    // entries belong to it.
    let mut folded: Vec<(Diag, DirMechanism, Vec<usize>, bool)> = Vec::new();
    let mut consumed: Vec<bool> = vec![false; hold.len()];

    for &mechanism in DirMechanism::ALL {
        let members: Vec<usize> = parsed
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                Some((m, ..)) if *m == mechanism => Some(i),
                _ => None,
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        // Count POLARITIES, not bindings: `["dpad.down", "DPAD.DOWN",
        // "dpad.right"]` is one V−, one H+ and folds (duplicates in a hold are
        // already harmless — `macros.rs`).
        let mut vertical: Option<bool> = None;
        let mut horizontal: Option<bool> = None;
        let mut split = false;
        let mut exact = true;
        for &i in &members {
            let (_, is_vertical, positive, entry_exact) = parsed[i].expect("filtered above");
            exact &= entry_exact;
            let slot = if is_vertical {
                &mut vertical
            } else {
                &mut horizontal
            };
            match slot {
                Some(seen) if *seen != positive => split = true,
                Some(_) => {}
                None => *slot = Some(positive),
            }
        }
        // FOLD IFF exactly one polarity on each axis. A contradictory bucket
        // (`down + forward + up`) is left as cardinals: guessing which diagonal
        // it meant would be fabrication, and what the pad publishes depends on
        // the slot's `socd` policy, which is resolved at plan time and is not
        // visible from here.
        let (Some(up), Some(right)) = (vertical, horizontal) else {
            continue;
        };
        if split {
            continue;
        }
        for &i in &members {
            consumed[i] = true;
        }
        folded.push((Diag::from_halves(up, right), mechanism, members, exact));
    }

    // Coalesce: buckets that folded to the SAME diagonal are ONE presented
    // control (the hat+stick double-binding).
    let mut out: Vec<Held> = Vec::new();
    for &diag in Diag::ALL {
        let hits: Vec<&(Diag, DirMechanism, Vec<usize>, bool)> =
            folded.iter().filter(|(d, ..)| *d == diag).collect();
        if hits.is_empty() {
            continue;
        }
        let mut members: Vec<usize> = hits.iter().flat_map(|(_, _, m, _)| m.clone()).collect();
        members.sort_unstable();
        out.push(Held::Diagonal {
            diag,
            mechanisms: hits.iter().map(|(_, m, _, _)| *m).collect(),
            members,
            exact: hits.iter().all(|(_, _, _, e)| *e),
        });
    }
    for (i, taken) in consumed.iter().enumerate() {
        if !taken {
            out.push(Held::Plain { member: i });
        }
    }
    out
}

/// Put the strings back. **Always exactly `hold`** — the view is a lens, and a
/// lens that could rewrite what it looked at would not be one.
///
/// It takes `view` so a caller that reordered or edited the presentation can
/// prove the round trip; the guarantee it returns is the original hold, in the
/// original order, byte for byte.
pub fn expand(hold: &[Binding], _view: &[Held]) -> Vec<Binding> {
    hold.to_vec()
}

/// The two bindings a diagonal is made of, on one mechanism — canonical
/// extremes, vertical first (the order a move list reads: ↓ then →).
///
/// This is the WRITE side of the lens: what "the user picked ↘" stores.
pub fn members_of(diag: Diag, mechanism: DirMechanism) -> [Binding; 2] {
    use crate::pad::{Axis, DpadDirection, AXIS_MAX, AXIS_MIN};
    let (up, right) = diag.halves();
    match mechanism {
        DirMechanism::Dpad => [
            Binding::Dpad(if up {
                DpadDirection::Up
            } else {
                DpadDirection::Down
            }),
            Binding::Dpad(if right {
                DpadDirection::Right
            } else {
                DpadDirection::Left
            }),
        ],
        DirMechanism::LeftStick => [
            Binding::Axis {
                axis: Axis::Y,
                value: if up { AXIS_MAX } else { AXIS_MIN },
            },
            Binding::Axis {
                axis: Axis::X,
                value: if right { AXIS_MAX } else { AXIS_MIN },
            },
        ],
        DirMechanism::RightStick => [
            Binding::Axis {
                axis: Axis::Ry,
                value: if up { AXIS_MAX } else { AXIS_MIN },
            },
            Binding::Axis {
                axis: Axis::Rx,
                value: if right { AXIS_MAX } else { AXIS_MIN },
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad::{Axis, DpadDirection, Trigger, XButton, AXIS_MAX, AXIS_MIN};
    use crate::socd::pointing;

    const fn dpad(d: DpadDirection) -> Binding {
        Binding::Dpad(d)
    }
    const fn axis(axis: Axis, value: i16) -> Binding {
        Binding::Axis { axis, value }
    }

    fn diags(view: &[Held]) -> Vec<Diag> {
        view.iter()
            .filter_map(|h| match h {
                Held::Diagonal { diag, .. } => Some(*diag),
                Held::Plain { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_documented_hadouken_middle_step_is_one_diagonal() {
        let hold = [dpad(DpadDirection::Down), dpad(DpadDirection::Right)];
        let view = fold(&hold);
        assert_eq!(
            view,
            vec![Held::Diagonal {
                diag: Diag::DownRight,
                mechanisms: vec![DirMechanism::Dpad],
                members: vec![0, 1],
                exact: true,
            }]
        );
        assert_eq!(expand(&hold, &view), hold.to_vec());
    }

    /// (a) — the single most common macro step in existence. Exact-pair
    /// matching on the whole step would fail it, which is what settles the rule.
    #[test]
    fn down_forward_and_a_button_folds_and_keeps_the_button() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            Binding::Button(XButton::A),
        ];
        assert_eq!(
            fold(&hold),
            vec![
                Held::Diagonal {
                    diag: Diag::DownRight,
                    mechanisms: vec![DirMechanism::Dpad],
                    members: vec![0, 1],
                    exact: true,
                },
                Held::Plain { member: 2 },
            ]
        );
    }

    /// (b) — contradictory. Which diagonal would it be? And what the pad
    /// publishes depends on the slot's `socd` policy, resolved at plan time.
    #[test]
    fn down_forward_up_never_folds() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            dpad(DpadDirection::Up),
        ];
        assert_eq!(
            fold(&hold),
            vec![
                Held::Plain { member: 0 },
                Held::Plain { member: 1 },
                Held::Plain { member: 2 },
            ]
        );
    }

    /// ⚠ **"CONTAINS BOTH HALVES" IS NOT "FOLDS", and the difference is a
    /// working grid.**
    ///
    /// It is the obvious shortcut for anything that has to answer *is this
    /// diagonal on?* — a mapper deciding whether a click on `↘` is a tick or an
    /// untick, a report deciding whether to mention one — and it is wrong on
    /// exactly the shape [`fold`] refuses: a mechanism holding BOTH polarities
    /// of an axis. `down + right + up` contains `dpad.down` and `dpad.right`,
    /// so contains-both says the diagonal is there; [`fold`] says it is not,
    /// and [`fold`] is right, because which diagonal it means depends on the
    /// slot's `socd` policy resolved at plan time.
    ///
    /// ksx-studio's macro grid shipped that shortcut in `macroToggleCell` and
    /// it did what the disagreement predicts: a `↘` cell painted OFF (by
    /// `fold`), whose own tooltip said "does not hold D-pad ↘", answered a
    /// click by taking the UNTICK branch — wiping all three holds, leaving the
    /// step empty and the cell still dark, and reporting that it had removed
    /// two holds when it removed three. In the one feature whose entire promise
    /// is that picking a diagonal gives you a diagonal.
    ///
    /// So the rule, stated where both mirrors read it: **ask `fold`.** The
    /// predicate that decides what a click does has to be the predicate that
    /// decided what the cell looks like, and there is only one of those.
    #[test]
    fn containing_both_halves_does_not_mean_it_folds() {
        let both = members_of(Diag::DownRight, DirMechanism::Dpad);
        let contradictory = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            dpad(DpadDirection::Up),
        ];
        // Contains-both says yes…
        assert!(both.iter().all(|b| contradictory.contains(b)));
        // …and the fold — the thing a grid is painted from — says no.
        assert!(diags(&fold(&contradictory)).is_empty());

        // Reachable without a hand-written file: tick ←, then →, then reach for
        // ↘. Every one of these contains both halves of some diagonal and folds
        // to nothing, on every mechanism.
        for &mechanism in DirMechanism::ALL {
            let [up, right] = members_of(Diag::UpRight, mechanism);
            let [down, left] = members_of(Diag::DownLeft, mechanism);
            for hold in [
                vec![down, left, right],
                vec![up, right, down],
                vec![down, right, up, left],
            ] {
                assert!(
                    diags(&fold(&hold)).is_empty(),
                    "{mechanism:?}: {hold:?} folded"
                );
            }
        }

        // The predicate that IS correct, and the one a mirror must re-derive:
        // does the view PRESENT this diagonal on this mechanism?
        let shown = |hold: &[Binding], diag: Diag, mechanism: DirMechanism| {
            fold(hold).iter().any(|h| match h {
                Held::Diagonal {
                    diag: d,
                    mechanisms,
                    ..
                } => *d == diag && mechanisms.contains(&mechanism),
                Held::Plain { .. } => false,
            })
        };
        assert!(!shown(&contradictory, Diag::DownRight, DirMechanism::Dpad));
        assert!(shown(&both, Diag::DownRight, DirMechanism::Dpad));
        assert!(!shown(&both, Diag::DownRight, DirMechanism::LeftStick));
    }

    #[test]
    fn duplicates_are_polarities_not_bindings() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
        ];
        assert_eq!(diags(&fold(&hold)), vec![Diag::DownRight]);
    }

    /// The hat+stick double-binding every in-box template writes: ONE presented
    /// diagonal naming both mechanisms, in canonical order.
    #[test]
    fn a_template_preset_folds_both_mechanisms_into_one_diagonal() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            axis(Axis::Y, AXIS_MIN),
            axis(Axis::X, AXIS_MAX),
        ];
        assert_eq!(
            fold(&hold),
            vec![Held::Diagonal {
                diag: Diag::DownRight,
                mechanisms: vec![DirMechanism::Dpad, DirMechanism::LeftStick],
                members: vec![0, 1, 2, 3],
                exact: true,
            }]
        );
    }

    /// Two mechanisms pointing DIFFERENT ways stay two presented diagonals.
    #[test]
    fn different_mechanisms_pointing_differently_stay_apart() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            axis(Axis::Ry, AXIS_MAX),
            axis(Axis::Rx, AXIS_MIN),
        ];
        assert_eq!(diags(&fold(&hold)), vec![Diag::UpLeft, Diag::DownRight]);
    }

    /// A contradiction on ONE mechanism never poisons another.
    #[test]
    fn a_contradictory_bucket_leaves_the_other_bucket_alone() {
        let hold = [
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Right),
            dpad(DpadDirection::Left),
            axis(Axis::Y, AXIS_MIN),
            axis(Axis::X, AXIS_MAX),
        ];
        let view = fold(&hold);
        assert_eq!(diags(&view), vec![Diag::DownRight]);
        match &view[0] {
            Held::Diagonal { mechanisms, .. } => {
                assert_eq!(mechanisms, &[DirMechanism::LeftStick]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(view.len(), 4, "the three dpad entries stay cardinal");
    }

    /// A centred axis is never part of a diagonal — the same rule `pointing`
    /// and `ksx_config::validate` state.
    #[test]
    fn a_centred_axis_is_never_half_of_a_diagonal() {
        let hold = [axis(Axis::Y, AXIS_MIN), axis(Axis::X, 0)];
        assert_eq!(
            fold(&hold),
            vec![Held::Plain { member: 0 }, Held::Plain { member: 1 }]
        );
    }

    /// A hand-written partial deflection still READS as the diagonal — it is
    /// only LABELLED as not-quite-canonical.
    #[test]
    fn a_partial_deflection_folds_but_is_marked_inexact() {
        let hold = [axis(Axis::Y, -16384), axis(Axis::X, AXIS_MAX)];
        assert_eq!(
            fold(&hold),
            vec![Held::Diagonal {
                diag: Diag::DownRight,
                mechanisms: vec![DirMechanism::LeftStick],
                members: vec![0, 1],
                exact: false,
            }]
        );
        assert_eq!(expand(&hold, &fold(&hold)), hold.to_vec());
    }

    #[test]
    fn passengers_are_never_folded() {
        let hold = [
            Binding::Button(XButton::A),
            Binding::Trigger(Trigger::Left),
            Binding::Consume,
        ];
        assert!(fold(&hold).iter().all(|h| matches!(h, Held::Plain { .. })));
    }

    #[test]
    fn an_empty_hold_presents_nothing() {
        assert!(fold(&[]).is_empty());
    }

    /// Round trip, exhaustively: whatever the view says, expansion is the hold.
    #[test]
    fn expansion_is_always_the_original_hold() {
        let vocabulary = [
            dpad(DpadDirection::Up),
            dpad(DpadDirection::Down),
            dpad(DpadDirection::Left),
            dpad(DpadDirection::Right),
            axis(Axis::X, AXIS_MIN),
            axis(Axis::X, AXIS_MAX),
            axis(Axis::Y, AXIS_MIN),
            axis(Axis::Y, AXIS_MAX),
            axis(Axis::X, 0),
            Binding::Button(XButton::A),
            Binding::Consume,
        ];
        for a in vocabulary {
            for b in vocabulary {
                for c in vocabulary {
                    let hold = [a, b, c];
                    let view = fold(&hold);
                    assert_eq!(expand(&hold, &view), hold.to_vec());
                    // Every index appears exactly once across the view.
                    let mut seen: Vec<usize> = view
                        .iter()
                        .flat_map(|h| match h {
                            Held::Diagonal { members, .. } => members.clone(),
                            Held::Plain { member } => vec![*member],
                        })
                        .collect();
                    seen.sort_unstable();
                    assert_eq!(seen, vec![0, 1, 2], "{hold:?} → {view:?}");
                }
            }
        }
    }

    /// The write side round-trips through the read side, for every diagonal on
    /// every mechanism.
    #[test]
    fn every_written_diagonal_reads_back_as_itself() {
        for &diag in Diag::ALL {
            for &mechanism in DirMechanism::ALL {
                let hold = members_of(diag, mechanism);
                assert_eq!(
                    fold(&hold),
                    vec![Held::Diagonal {
                        diag,
                        mechanisms: vec![mechanism],
                        members: vec![0, 1],
                        exact: true,
                    }],
                    "{diag:?} on {mechanism:?}"
                );
            }
        }
    }

    /// ⚠ THE SIGN TRAP, pinned. "Up" on a stick is `AXIS_MAX`, NOT `AXIS_MIN`
    /// (XInput's positive Y is up; legacy binds Up to `AXIS_MAX`). A
    /// copy-paste of the down-diagonals with the sign wrong yields an ↖ that
    /// reads back perfectly in every UI and does nothing in the game — the
    /// worst failure mode this feature has, because nothing catches it.
    ///
    /// So the two UP diagonals are asserted VALUE BY VALUE, on all three
    /// mechanisms, rather than only through the round trip (which a
    /// consistently-wrong sign would pass).
    #[test]
    fn the_up_diagonals_deflect_upwards_on_every_mechanism() {
        assert_eq!(
            members_of(Diag::UpLeft, DirMechanism::LeftStick),
            [axis(Axis::Y, AXIS_MAX), axis(Axis::X, AXIS_MIN)]
        );
        assert_eq!(
            members_of(Diag::UpRight, DirMechanism::LeftStick),
            [axis(Axis::Y, AXIS_MAX), axis(Axis::X, AXIS_MAX)]
        );
        assert_eq!(
            members_of(Diag::UpLeft, DirMechanism::RightStick),
            [axis(Axis::Ry, AXIS_MAX), axis(Axis::Rx, AXIS_MIN)]
        );
        assert_eq!(
            members_of(Diag::UpRight, DirMechanism::RightStick),
            [axis(Axis::Ry, AXIS_MAX), axis(Axis::Rx, AXIS_MAX)]
        );
        // …and the DOWN ones, so "both signs" is a fact and not an assumption.
        assert_eq!(
            members_of(Diag::DownLeft, DirMechanism::LeftStick),
            [axis(Axis::Y, AXIS_MIN), axis(Axis::X, AXIS_MIN)]
        );
        assert_eq!(
            members_of(Diag::DownRight, DirMechanism::RightStick),
            [axis(Axis::Ry, AXIS_MIN), axis(Axis::Rx, AXIS_MAX)]
        );
        // The dpad has no sign to get wrong, but it has a NAME to get wrong.
        assert_eq!(
            members_of(Diag::UpLeft, DirMechanism::Dpad),
            [dpad(DpadDirection::Up), dpad(DpadDirection::Left)]
        );
        assert_eq!(
            members_of(Diag::DownRight, DirMechanism::Dpad),
            [dpad(DpadDirection::Down), dpad(DpadDirection::Right)]
        );

        // Read-back, stated the same way round: an UP-deflected stick is an UP
        // diagonal. This is the assertion a mirrored sign fails.
        assert_eq!(
            fold(&[axis(Axis::Y, AXIS_MAX), axis(Axis::X, AXIS_MIN)]),
            vec![Held::Diagonal {
                diag: Diag::UpLeft,
                mechanisms: vec![DirMechanism::LeftStick],
                members: vec![0, 1],
                exact: true,
            }]
        );
        assert_eq!(
            fold(&[axis(Axis::Y, AXIS_MIN), axis(Axis::X, AXIS_MIN)]),
            vec![Held::Diagonal {
                diag: Diag::DownLeft,
                mechanisms: vec![DirMechanism::LeftStick],
                members: vec![0, 1],
                exact: true,
            }]
        );
        // ALL FOUR walk the gate. A 360 needs every one of them, which is why
        // none of them may be second class.
        let gate = [
            (Diag::DownRight, false, true),
            (Diag::DownLeft, false, false),
            (Diag::UpLeft, true, false),
            (Diag::UpRight, true, true),
        ];
        for (diag, up, right) in gate {
            assert_eq!(diag.halves(), (up, right), "{diag:?}");
            for &mechanism in DirMechanism::ALL {
                let pair = members_of(diag, mechanism);
                let v = pointing(pair[0]).expect("vertical half points");
                let h = pointing(pair[1]).expect("horizontal half points");
                assert!(v.vertical && !h.vertical, "{diag:?} {mechanism:?}");
                assert_eq!(v.positive, up, "{diag:?} {mechanism:?} vertical sign");
                assert_eq!(h.positive, right, "{diag:?} {mechanism:?} horizontal sign");
            }
        }
    }

    #[test]
    fn the_notation_table_is_the_one_in_the_docs() {
        assert_eq!(
            Diag::ALL.iter().map(|d| d.numpad()).collect::<Vec<_>>(),
            vec![7, 9, 1, 3]
        );
        for &d in Diag::ALL {
            assert_eq!(Diag::from_halves(d.halves().0, d.halves().1), d);
        }
        assert_eq!(Diag::DownRight.glyph(), "↘");
        assert_eq!(Diag::DownRight.words(), "down-right");
        assert_eq!(Diag::DownRight.move_list(), "d/f");
    }
}
