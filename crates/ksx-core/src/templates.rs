//! In-box preset templates — ready-made layouts for the panels people
//! actually own, so a fresh install has something that WORKS before anyone
//! opens a mapper.
//!
//! This is commandment 9 of `docs/MAPPER-UX.md` ("the best mapping session is
//! none") turned into code: an I-PAC ships MAME-ready, RetroBat compiles one
//! mapping into 130 emulators, and a person who bought a standard 6-button
//! panel should not have to press 20 buttons in a wizard to describe a layout
//! the whole arcade world already agrees on. The wizard (`ksx setup`) exists
//! for the exceptions, not for the common case.
//!
//! # Why the keys are the ones they are
//!
//! Every arcade template uses the **factory / MAME default keyboard chart** —
//! what an unprogrammed I-PAC sends out of the box, which is also what MAME
//! has read for thirty years. That is the whole point: match the hardware's
//! defaults and nobody has to touch anything.
//!
//! # Directions drive the D-pad AND the left stick
//!
//! Every direction row appears twice — once as `dpad.*`, once as an axis
//! extreme. Some games read only the hat, some read only the stick, and
//! keyboard fan-out (one key → several outputs) is native to the engine
//! (`docs/INPUT-TRANSFORMS.md` §1a), so binding both costs nothing and removes
//! an entire category of "my stick does nothing in this game". It is a
//! feature, not a duplicate: `ksx map --function dpad.up --clear` removes half
//! of it if a game ever wants only the stick.
//!
//! # Templates are seeds, not built-ins
//!
//! [`Preset::builtin_default`] and [`Preset::builtin_empty`] are PROTECTED —
//! they live in code and cannot be edited. A template is instantiated into an
//! ordinary user preset file (`protected: false`) under whatever name the user
//! asks for, and from then on it is theirs. `default` and `empty` are listed
//! here too so that "start from the legacy layout" is the same verb as "start
//! from a 6-button panel".

use core::fmt;

use crate::key::Key;
use crate::pad::{Axis, DpadDirection, Trigger, XButton, AXIS_MAX, AXIS_MIN};
use crate::preset::{Binding, Preset};

/// How many player blocks a template can carry.
pub const MAX_TEMPLATE_PLAYERS: usize = 4;

/// One row of a template: an output, and the key that drives it for each
/// player block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub binding: Binding,
    /// Key per player, `players[0]` = player 1. [`Key::None`] means this
    /// player's factory chart has no key for that control — the row is simply
    /// not emitted for them (an inert placeholder would only be noise in a
    /// template that is meant to be read).
    pub keys: [Key; MAX_TEMPLATE_PLAYERS],
}

const fn row(binding: Binding, keys: [Key; MAX_TEMPLATE_PLAYERS]) -> Row {
    Row { binding, keys }
}

const fn button(b: XButton) -> Binding {
    Binding::Button(b)
}

const fn trigger(t: Trigger) -> Binding {
    Binding::Trigger(t)
}

const fn dpad(d: DpadDirection) -> Binding {
    Binding::Dpad(d)
}

const fn axis(axis: Axis, value: i16) -> Binding {
    Binding::Axis { axis, value }
}

/// Where a template's rows come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    /// A per-player key table.
    Rows(&'static [Row]),
    /// [`Preset::builtin_default`], renamed and unprotected.
    Default,
    /// [`Preset::builtin_empty`], renamed and unprotected.
    Empty,
}

/// A named starting point for a preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// Stable id, the spelling `--from-template` takes.
    pub id: &'static str,
    /// One line, for `ksx preset list --templates`.
    pub summary: &'static str,
    /// The panel this is FOR, and what each control does on it. Printed by
    /// `ksx preset list --templates --json` and by the docs — a template
    /// nobody can identify from a list is a template nobody uses.
    pub panel: &'static str,
    /// How many player blocks the table carries (`--player 1..=players`).
    pub players: u8,
    body: Body,
}

/// Why a template could not be instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// No template with that id.
    Unknown(String),
    /// `--player N` outside the table.
    NoSuchPlayer {
        id: &'static str,
        asked: u8,
        players: u8,
    },
    /// A preset must have a name to be stored under.
    EmptyName,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => {
                write!(f, "no in-box template called \"{id}\"")
            }
            Self::NoSuchPlayer { id, asked, players } => write!(
                f,
                "template \"{id}\" carries {players} player block(s); asked for player {asked}"
            ),
            Self::EmptyName => write!(f, "a preset needs a name"),
        }
    }
}

impl std::error::Error for TemplateError {}

impl Template {
    /// Build a real, editable preset from this template.
    ///
    /// `player` selects the key block (1-based). Rows with [`Key::None`] for
    /// that player are dropped, so the file holds only what the panel can
    /// actually reach.
    pub fn instantiate(&self, name: &str, player: u8) -> Result<Preset, TemplateError> {
        if name.trim().is_empty() {
            return Err(TemplateError::EmptyName);
        }
        if player < 1 || player > self.players {
            return Err(TemplateError::NoSuchPlayer {
                id: self.id,
                asked: player,
                players: self.players,
            });
        }
        let mut preset = match self.body {
            Body::Rows(rows) => {
                let index = usize::from(player - 1);
                let entries = rows
                    .iter()
                    .filter_map(|r| {
                        let key = r.keys[index];
                        (key != Key::None).then_some((key, r.binding))
                    })
                    .collect();
                Preset {
                    name: String::new(),
                    entries,
                    chords: Vec::new(),
                    macros: Default::default(),
                    turbo: Vec::new(),
                    protected: false,
                }
            }
            Body::Default => Preset::builtin_default(),
            Body::Empty => Preset::builtin_empty(),
        };
        // A template is a SEED: whatever it came from, the result is an
        // ordinary user preset the mapper may edit.
        preset.name = name.to_owned();
        preset.protected = false;
        Ok(preset)
    }

    /// The distinct keys this template's player block uses, in row order —
    /// what the docs print as "the panel it expects".
    pub fn keys_for(&self, player: u8) -> Vec<Key> {
        let Body::Rows(rows) = self.body else {
            return Vec::new();
        };
        if player < 1 || player > self.players {
            return Vec::new();
        }
        let index = usize::from(player - 1);
        let mut keys: Vec<Key> = Vec::new();
        for r in rows {
            let key = r.keys[index];
            if key != Key::None && !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }
}

/// **`arcade-6button`** — the I-PAC2 / MAME six-button fighting panel.
///
/// Buttons in MAME's row order: 1·2·3 on the top row, 4·5·6 below, which on a
/// Street-Fighter panel is LP·MP·HP over LK·MK·HK. The mapping to an Xbox pad
/// is the one every fightstick ships with: `X Y RB` over `A B RT`.
///
/// Buttons 7 and 8 (`C`/`V` for P1, `J`/`L` for P2) land on LB and LT — they
/// exist on plenty of panels and cost nothing when they do not.
const ARCADE_6BUTTON: &[Row] = &[
    // Stick: hat + left stick together.
    row(
        dpad(DpadDirection::Up),
        [Key::Up, Key::R, Key::None, Key::None],
    ),
    row(
        axis(Axis::Y, AXIS_MAX),
        [Key::Up, Key::R, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Down),
        [Key::Down, Key::F, Key::None, Key::None],
    ),
    row(
        axis(Axis::Y, AXIS_MIN),
        [Key::Down, Key::F, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Left),
        [Key::Left, Key::D, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MIN),
        [Key::Left, Key::D, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Right),
        [Key::Right, Key::G, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MAX),
        [Key::Right, Key::G, Key::None, Key::None],
    ),
    // Top row: buttons 1·2·3 (light/medium/heavy punch).
    row(
        button(XButton::X),
        [Key::LeftControl, Key::A, Key::None, Key::None],
    ),
    row(
        button(XButton::Y),
        [Key::LeftAlt, Key::S, Key::None, Key::None],
    ),
    row(
        button(XButton::RightBumper),
        [Key::Space, Key::Q, Key::None, Key::None],
    ),
    // Bottom row: buttons 4·5·6 (light/medium/heavy kick).
    row(
        button(XButton::A),
        [Key::LeftShift, Key::W, Key::None, Key::None],
    ),
    row(button(XButton::B), [Key::Z, Key::I, Key::None, Key::None]),
    row(
        trigger(Trigger::Right),
        [Key::X, Key::K, Key::None, Key::None],
    ),
    // Buttons 7·8, if the panel has them.
    row(
        button(XButton::LeftBumper),
        [Key::C, Key::J, Key::None, Key::None],
    ),
    row(
        trigger(Trigger::Left),
        [Key::V, Key::L, Key::None, Key::None],
    ),
    // Start and coin — the cabinet's exit keys.
    row(
        button(XButton::Start),
        [Key::One, Key::Two, Key::None, Key::None],
    ),
    row(
        button(XButton::Back),
        [Key::Five, Key::Six, Key::None, Key::None],
    ),
];

/// **`arcade-4way`** — the four-player, two-button cabinet (Gauntlet, TMNT,
/// Pac-Man class), on MAME's four-player default chart.
///
/// MAME only ever defaulted three buttons for players 3 and 4, and a 4-player
/// panel is a 2- or 3-button panel in practice — so this is deliberately
/// small: stick, two buttons, start, coin. Anything more is `ksx map` or the
/// wizard.
const ARCADE_4WAY: &[Row] = &[
    row(
        dpad(DpadDirection::Up),
        [Key::Up, Key::R, Key::I, Key::Numpad8],
    ),
    row(
        axis(Axis::Y, AXIS_MAX),
        [Key::Up, Key::R, Key::I, Key::Numpad8],
    ),
    row(
        dpad(DpadDirection::Down),
        [Key::Down, Key::F, Key::K, Key::Numpad2],
    ),
    row(
        axis(Axis::Y, AXIS_MIN),
        [Key::Down, Key::F, Key::K, Key::Numpad2],
    ),
    row(
        dpad(DpadDirection::Left),
        [Key::Left, Key::D, Key::J, Key::Numpad4],
    ),
    row(
        axis(Axis::X, AXIS_MIN),
        [Key::Left, Key::D, Key::J, Key::Numpad4],
    ),
    row(
        dpad(DpadDirection::Right),
        [Key::Right, Key::G, Key::L, Key::Numpad6],
    ),
    row(
        axis(Axis::X, AXIS_MAX),
        [Key::Right, Key::G, Key::L, Key::Numpad6],
    ),
    row(
        button(XButton::A),
        [Key::LeftControl, Key::A, Key::RightControl, Key::Numpad0],
    ),
    row(
        button(XButton::B),
        [Key::LeftAlt, Key::S, Key::RightShift, Key::NumpadDelete],
    ),
    row(
        button(XButton::Start),
        [Key::One, Key::Two, Key::Three, Key::Four],
    ),
    row(
        button(XButton::Back),
        [Key::Five, Key::Six, Key::Seven, Key::Eight],
    ),
];

/// **`keyboard-wasd`** — one modern desktop keyboard, one pad.
///
/// The layout a PC player already has in their hands: WASD is the left stick,
/// the arrow cluster is the right stick, and the face buttons sit where their
/// verbs do (Space jumps, C crouches, R reloads, F interacts). The numpad
/// carries the D-pad, which desktop games use for menus and weapon wheels.
///
/// One block only, and that is not a limitation: bindings are per-slot, so two
/// people on two keyboards each get this template on their own device — the
/// T2 "two people, one PC" case from `docs/USE-CASES.md`, with no key overlap
/// to reason about.
const KEYBOARD_WASD: &[Row] = &[
    // Left stick — movement.
    row(
        axis(Axis::Y, AXIS_MAX),
        [Key::W, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::Y, AXIS_MIN),
        [Key::S, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MIN),
        [Key::A, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MAX),
        [Key::D, Key::None, Key::None, Key::None],
    ),
    // Right stick — camera.
    row(
        axis(Axis::Ry, AXIS_MAX),
        [Key::Up, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::Ry, AXIS_MIN),
        [Key::Down, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::Rx, AXIS_MIN),
        [Key::Left, Key::None, Key::None, Key::None],
    ),
    row(
        axis(Axis::Rx, AXIS_MAX),
        [Key::Right, Key::None, Key::None, Key::None],
    ),
    // D-pad — menus, weapon wheels.
    row(
        dpad(DpadDirection::Up),
        [Key::Numpad8, Key::None, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Down),
        [Key::Numpad2, Key::None, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Left),
        [Key::Numpad4, Key::None, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Right),
        [Key::Numpad6, Key::None, Key::None, Key::None],
    ),
    // Face buttons, where their verbs live.
    row(
        button(XButton::A),
        [Key::Space, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::B),
        [Key::C, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::X),
        [Key::R, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::Y),
        [Key::F, Key::None, Key::None, Key::None],
    ),
    // Shoulders and triggers, both hands' reach from WASD.
    row(
        button(XButton::LeftBumper),
        [Key::Q, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::RightBumper),
        [Key::E, Key::None, Key::None, Key::None],
    ),
    row(
        trigger(Trigger::Left),
        [Key::Z, Key::None, Key::None, Key::None],
    ),
    row(
        trigger(Trigger::Right),
        [Key::X, Key::None, Key::None, Key::None],
    ),
    // Thumbs: sprint and melee, the usual homes.
    row(
        button(XButton::LeftThumb),
        [Key::LeftShift, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::RightThumb),
        [Key::V, Key::None, Key::None, Key::None],
    ),
    // System.
    row(
        button(XButton::Start),
        [Key::Enter, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::Back),
        [Key::Backspace, Key::None, Key::None, Key::None],
    ),
    row(
        button(XButton::Guide),
        [Key::LeftWindows, Key::None, Key::None, Key::None],
    ),
];

/// **`keyboard-2p`** — ONE keyboard, TWO players, no encoder.
///
/// The couch case `keyboard-wasd` cannot serve: two people, one Steam game, one
/// keyboard between them. `keyboard-wasd` hands a single player the whole board
/// — WASD *and* the arrows *and* the numpad — so instantiating it for two slots
/// would drive both pads off the same keys.
///
/// The split is by HAND POSITION, not by taste, because the two players are
/// sitting elbow to elbow. Player 2's entire block lives past the right edge of
/// the letters: their left hand on the arrow cluster, their right hand on the
/// numpad, neither of them over player 1's keys. Player 1 keeps the left hand's
/// own territory — WASD and the letters that ring it.
///
/// Player 1's buttons are `keyboard-wasd`'s, key for key (Space=A, C=B, R=X,
/// F=Y, Q/E, Z/X, LeftShift, V), so anyone who learned that template keeps
/// their fingers. Only Start and Back move: `keyboard-wasd` puts them on Enter
/// and Backspace, which sit inches from player 2's arrow hand.
///
/// Player 2's numpad IS a pad face — 8·4·6·2 is the Y·X·B·A diamond in the same
/// geometry as the buttons on the controller, and the four corners of that 3×3
/// are the shoulders, bumpers above triggers the way they stack on the pad. The
/// whole block is one hand span, thumb on Numpad0 and NumpadEnter.
///
/// Neither block carries a right stick: half a keyboard has no second four-key
/// cluster inside one hand's reach. The games two people share a keyboard for —
/// platformers, fighters, sports, party games — read the left stick and the
/// hat, and both are bound.
const KEYBOARD_2P: &[Row] = &[
    // Movement drives the hat AND the left stick. Unlike `keyboard-wasd` there
    // is no spare cluster to put the other one on, so a game that reads only
    // the hat and a game that reads only the stick both have to work off these
    // four keys.
    row(
        dpad(DpadDirection::Up),
        [Key::W, Key::Up, Key::None, Key::None],
    ),
    row(
        axis(Axis::Y, AXIS_MAX),
        [Key::W, Key::Up, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Down),
        [Key::S, Key::Down, Key::None, Key::None],
    ),
    row(
        axis(Axis::Y, AXIS_MIN),
        [Key::S, Key::Down, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Left),
        [Key::A, Key::Left, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MIN),
        [Key::A, Key::Left, Key::None, Key::None],
    ),
    row(
        dpad(DpadDirection::Right),
        [Key::D, Key::Right, Key::None, Key::None],
    ),
    row(
        axis(Axis::X, AXIS_MAX),
        [Key::D, Key::Right, Key::None, Key::None],
    ),
    // Face buttons: P1 keeps the verbs from `keyboard-wasd`; P2 gets the numpad
    // diamond, which already has the pad's own A-bottom / Y-top geometry.
    row(
        button(XButton::A),
        [Key::Space, Key::Numpad2, Key::None, Key::None],
    ),
    row(
        button(XButton::B),
        [Key::C, Key::Numpad6, Key::None, Key::None],
    ),
    row(
        button(XButton::X),
        [Key::R, Key::Numpad4, Key::None, Key::None],
    ),
    row(
        button(XButton::Y),
        [Key::F, Key::Numpad8, Key::None, Key::None],
    ),
    // Shoulders: the corners of P2's 3×3, bumpers on the top row and triggers
    // on the bottom, so the hand never leaves the block it holds the face
    // buttons with.
    row(
        button(XButton::LeftBumper),
        [Key::Q, Key::Numpad7, Key::None, Key::None],
    ),
    row(
        button(XButton::RightBumper),
        [Key::E, Key::Numpad9, Key::None, Key::None],
    ),
    row(
        trigger(Trigger::Left),
        [Key::Z, Key::Numpad1, Key::None, Key::None],
    ),
    row(
        trigger(Trigger::Right),
        [Key::X, Key::Numpad3, Key::None, Key::None],
    ),
    // Thumbs: sprint goes on the Shift each player's hand is already resting
    // against — LeftShift under P1's pinky at WASD, RightShift just above P2's
    // arrow cluster.
    row(
        button(XButton::LeftThumb),
        [Key::LeftShift, Key::RightShift, Key::None, Key::None],
    ),
    row(
        button(XButton::RightThumb),
        [Key::V, Key::Numpad5, Key::None, Key::None],
    ),
    // Start and Back stay inside each player's own half: pausing must not mean
    // reaching across the other player's hands, which Enter and Backspace
    // (`keyboard-wasd`'s pair) would for P1 and Escape would for P2. P1's Tab
    // is the PC convention for Back and the one key their pinky already rests
    // beside; the price is that Alt+Tab is consumed while this preset is
    // loaded, which `ksx map --function back --key G` buys back.
    row(
        button(XButton::Start),
        [Key::One, Key::NumpadEnter, Key::None, Key::None],
    ),
    row(
        button(XButton::Back),
        [Key::Tab, Key::Numpad0, Key::None, Key::None],
    ),
];

/// Every in-box template, in the order `ksx preset list --templates` prints
/// them: the panels first, the two built-in floors last.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "arcade-6button",
        summary: "I-PAC / MAME six-button fighting panel (2 players)",
        panel: "A standard two-player, six-button arcade panel on an I-PAC or any \
encoder still on its factory chart. Stick drives the D-pad and the left stick; \
the top button row is X · Y · RB (light/medium/heavy punch), the bottom row is \
A · B · RT (light/medium/heavy kick); buttons 7-8, if wired, are LB and LT. \
Player 1 uses the arrow keys with LeftControl/LeftAlt/Space/LeftShift/Z/X and \
1 (start) / 5 (coin); player 2 uses R·F·D·G with A/S/Q/W/I/K and 2 / 6.",
        players: 2,
        body: Body::Rows(ARCADE_6BUTTON),
    },
    Template {
        id: "arcade-4way",
        summary: "Four-way stick, two buttons, up to 4 players (MAME chart)",
        panel: "A four-player cabinet on MAME's default keyboard chart: four-way \
stick plus two buttons per player. Stick drives the D-pad and the left stick, \
button 1 is A, button 2 is B, start and coin are the numbered keys \
(1-4 / 5-8). P1 arrows + LeftControl/LeftAlt, P2 R·F·D·G + A/S, \
P3 I·K·J·L + RightControl/RightShift, P4 numpad 8·2·4·6 + Numpad0/Numpad-Del.",
        players: 4,
        body: Body::Rows(ARCADE_4WAY),
    },
    Template {
        id: "keyboard-wasd",
        summary: "Modern desktop keyboard: WASD moves, arrows aim",
        panel: "One ordinary keyboard, one pad. WASD is the left stick, the arrow \
cluster is the right stick, the numpad is the D-pad. Space=A, C=B, R=X, F=Y, \
Q/E shoulders, Z/X triggers, LeftShift/V thumbsticks, Enter=Start, \
Backspace=Back, LeftWindows=Guide. Give each player their own keyboard and \
use this template for all of them — bindings are per-slot, so nothing \
collides.",
        players: 1,
        body: Body::Rows(KEYBOARD_WASD),
    },
    Template {
        id: "keyboard-2p",
        summary: "Two players sharing ONE keyboard: WASD vs the arrows",
        panel: "Two people on one ordinary keyboard, no encoder — the couch \
co-op case. Player 1 keeps the left hand on WASD (hat AND left stick) with \
Space=A, C=B, R=X, F=Y, Q/E bumpers, Z/X triggers, LeftShift/V thumbsticks, \
1=Start, Tab=Back. Player 2 sits to the right of the letters: the arrows are \
the hat and left stick, the numpad is the pad face (Numpad 8·4·6·2 = Y·X·B·A) \
with Numpad7/9 bumpers, Numpad1/3 triggers, RightShift/Numpad5 thumbsticks, \
NumpadEnter=Start, Numpad0=Back. The two blocks share no key, so one press \
moves one player. Neither has a right stick — there is no room for one on half \
a keyboard; use keyboard-wasd on a keyboard each when a game needs it.",
        players: 2,
        body: Body::Rows(KEYBOARD_2P),
    },
    Template {
        id: "default",
        summary: "The ported legacy default layout (S=A, D=B, A=X, W=Y)",
        panel: "The layout the legacy Keyboard Splitter shipped, kept for people \
migrating from it: S/D/A/W face buttons, Q/E triggers, arrow keys on the left \
stick, numpad 4/6/2/8 on the right stick, I/K/J/L D-pad, Escape=Start, \
Backspace=Back. This is also what `ksx map --restore defaults` writes.",
        players: 1,
        body: Body::Default,
    },
    Template {
        id: "empty",
        summary: "Every control listed, nothing bound — a blank worksheet",
        panel: "No bindings at all: every function is present with the inert \
\"None\" placeholder, so the file (and the mapper) shows the whole control \
surface with nothing on it. The right starting point when a panel matches no \
chart and you intend to map it yourself.",
        players: 1,
        body: Body::Empty,
    },
];

/// Look a template up by id, case-insensitively (`Arcade-6Button` works).
pub fn find(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id.eq_ignore_ascii_case(id))
}

/// Instantiate by id — the one call `ksx preset new --from-template` makes.
pub fn instantiate(id: &str, name: &str, player: u8) -> Result<Preset, TemplateError> {
    find(id)
        .ok_or_else(|| TemplateError::Unknown(id.to_owned()))?
        .instantiate(name, player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn keys_for(preset: &Preset, binding: Binding) -> Vec<Key> {
        preset
            .entries
            .iter()
            .filter(|(_, b)| *b == binding)
            .map(|(k, _)| *k)
            .collect()
    }

    #[test]
    fn every_template_instantiates_for_every_player_it_claims() {
        for template in TEMPLATES {
            for player in 1..=template.players {
                let preset = template
                    .instantiate("Try", player)
                    .unwrap_or_else(|e| panic!("{} p{player}: {e}", template.id));
                assert_eq!(preset.name, "Try");
                assert!(
                    !preset.protected,
                    "{} is a seed, not a built-in",
                    template.id
                );
                assert!(preset.chords.is_empty());
                assert!(preset.macros.is_empty());
                assert!(preset.turbo.is_empty());
            }
            assert!(template.instantiate("Try", template.players + 1).is_err());
            assert!(template.instantiate("Try", 0).is_err());
        }
    }

    #[test]
    fn ids_are_unique_and_findable_case_insensitively() {
        let ids: BTreeSet<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), TEMPLATES.len());
        assert_eq!(find("ARCADE-6BUTTON").map(|t| t.id), Some("arcade-6button"));
        assert!(find("no-such-panel").is_none());
        assert!(matches!(
            instantiate("no-such-panel", "X", 1),
            Err(TemplateError::Unknown(_))
        ));
    }

    #[test]
    fn a_nameless_preset_is_refused() {
        assert_eq!(
            instantiate("arcade-6button", "   ", 1),
            Err(TemplateError::EmptyName)
        );
    }

    /// No template may bind one key to two DIFFERENT physical controls by
    /// accident. A key legitimately appears twice — hat + stick for one stick
    /// direction — so the rule is per (key, "control group"), which here means:
    /// a key may drive several outputs only if they are the same direction.
    #[test]
    fn no_template_binds_a_key_to_two_unrelated_controls() {
        for template in TEMPLATES {
            for player in 1..=template.players {
                let preset = template.instantiate("Try", player).unwrap();
                for key in preset.entries.iter().map(|(k, _)| *k) {
                    if key == Key::None {
                        continue;
                    }
                    let drives: Vec<Binding> = preset
                        .entries
                        .iter()
                        .filter(|(k, _)| *k == key)
                        .map(|(_, b)| *b)
                        .collect();
                    assert!(
                        drives.len() <= 2,
                        "{} p{player}: {} drives {drives:?}",
                        template.id,
                        key.name()
                    );
                    if drives.len() == 2 {
                        let hat = drives.iter().any(|b| matches!(b, Binding::Dpad(_)));
                        let stick = drives.iter().any(|b| matches!(b, Binding::Axis { .. }));
                        assert!(
                            hat && stick,
                            "{} p{player}: {} drives {drives:?}, which is not a \
                             direction's hat+stick pair",
                            template.id,
                            key.name()
                        );
                    }
                }
            }
        }
    }

    /// The audit the wizard performs, applied to what we ship: a template that
    /// cannot reach Start or Back strands a cabinet.
    #[test]
    fn every_panel_template_can_reach_start_and_back() {
        for template in TEMPLATES {
            if matches!(template.body, Body::Empty) {
                continue; // the blank worksheet binds nothing, by definition
            }
            for player in 1..=template.players {
                let preset = template.instantiate("Try", player).unwrap();
                assert!(
                    !keys_for(&preset, Binding::Button(XButton::Start)).is_empty(),
                    "{} p{player} has no Start",
                    template.id
                );
                assert!(
                    !keys_for(&preset, Binding::Button(XButton::Back)).is_empty(),
                    "{} p{player} has no Back",
                    template.id
                );
            }
        }
    }

    /// Every direction of every panel template drives the hat AND the stick.
    #[test]
    fn directions_drive_both_the_hat_and_the_stick() {
        for id in ["arcade-6button", "arcade-4way", "keyboard-2p"] {
            let template = find(id).unwrap();
            for player in 1..=template.players {
                let preset = template.instantiate("Try", player).unwrap();
                for (dpad_dir, axis, value) in [
                    (DpadDirection::Up, Axis::Y, AXIS_MAX),
                    (DpadDirection::Down, Axis::Y, AXIS_MIN),
                    (DpadDirection::Left, Axis::X, AXIS_MIN),
                    (DpadDirection::Right, Axis::X, AXIS_MAX),
                ] {
                    let hat = keys_for(&preset, Binding::Dpad(dpad_dir));
                    let stick = keys_for(&preset, Binding::Axis { axis, value });
                    assert_eq!(hat, stick, "{id} p{player} {dpad_dir:?}");
                    assert_eq!(hat.len(), 1, "{id} p{player} {dpad_dir:?}");
                }
            }
        }
    }

    /// Player blocks must not overlap: two players on ONE encoder is the
    /// primary topology, so P1 and P2 sharing a key would make one panel drive
    /// two pads.
    #[test]
    fn player_blocks_never_share_a_key() {
        for template in TEMPLATES {
            for a in 1..=template.players {
                for b in (a + 1)..=template.players {
                    let first: BTreeSet<Key> = template.keys_for(a).into_iter().collect();
                    let second: BTreeSet<Key> = template.keys_for(b).into_iter().collect();
                    let shared: Vec<&Key> = first.intersection(&second).collect();
                    assert!(
                        shared.is_empty(),
                        "{} p{a}/p{b} share {shared:?}",
                        template.id
                    );
                }
            }
        }
    }

    #[test]
    fn the_six_button_layout_is_the_fightstick_one() {
        let preset = find("arcade-6button")
            .unwrap()
            .instantiate("P1", 1)
            .unwrap();
        // Top row: buttons 1-2-3 = LP MP HP = X Y RB.
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::X)),
            vec![Key::LeftControl]
        );
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::Y)),
            vec![Key::LeftAlt]
        );
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::RightBumper)),
            vec![Key::Space]
        );
        // Bottom row: buttons 4-5-6 = LK MK HK = A B RT.
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::A)),
            vec![Key::LeftShift]
        );
        assert_eq!(keys_for(&preset, Binding::Button(XButton::B)), vec![Key::Z]);
        assert_eq!(
            keys_for(&preset, Binding::Trigger(Trigger::Right)),
            vec![Key::X]
        );
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::Start)),
            vec![Key::One]
        );
        assert_eq!(
            keys_for(&preset, Binding::Button(XButton::Back)),
            vec![Key::Five]
        );

        let p2 = find("arcade-6button")
            .unwrap()
            .instantiate("P2", 2)
            .unwrap();
        assert_eq!(keys_for(&p2, Binding::Button(XButton::X)), vec![Key::A]);
        assert_eq!(
            keys_for(&p2, Binding::Button(XButton::Start)),
            vec![Key::Two]
        );
    }

    /// `keyboard-2p` is the one template where two people press the SAME
    /// physical keyboard, so a key in both blocks would move both players at
    /// once. Overlap is checked for every template; what this pins is WHERE the
    /// keys are — player 2's whole block has to sit past the right edge of the
    /// letters (arrow cluster, numpad, RightShift) or their hands land on
    /// player 1's, which no amount of reading the table by eye catches.
    #[test]
    fn the_two_player_keyboard_keeps_each_player_out_of_the_others_half() {
        let template = find("keyboard-2p").unwrap();
        let p1: BTreeSet<Key> = template.keys_for(1).into_iter().collect();
        let p2: BTreeSet<Key> = template.keys_for(2).into_iter().collect();
        let shared: Vec<&Key> = p1.intersection(&p2).collect();
        assert!(
            shared.is_empty(),
            "one press would move both players: {shared:?}"
        );

        // Player 2's territory, defined by the keyboard and not by the table
        // above: the clusters right of the letters, plus the right-hand
        // modifiers that border them. Spelled out in full so that moving a
        // control onto some *other* numpad or nav key still trips the check.
        let right_of_the_letters = |key: Key| {
            matches!(
                key,
                Key::Up
                    | Key::Down
                    | Key::Left
                    | Key::Right
                    | Key::Home
                    | Key::End
                    | Key::PageUp
                    | Key::PageDown
                    | Key::Insert
                    | Key::Delete
                    | Key::NumLock
                    | Key::NumpadDivide
                    | Key::NumpadAsterisk
                    | Key::NumpadMinus
                    | Key::NumpadPlus
                    | Key::NumpadEnter
                    | Key::NumpadDelete
                    | Key::Numpad0
                    | Key::Numpad1
                    | Key::Numpad2
                    | Key::Numpad3
                    | Key::Numpad4
                    | Key::Numpad5
                    | Key::Numpad6
                    | Key::Numpad7
                    | Key::Numpad8
                    | Key::Numpad9
                    | Key::RightShift
                    | Key::RightControl
                    | Key::RightAlt
                    | Key::RightWindows
                    | Key::Menu
            )
        };
        for key in &p2 {
            assert!(
                right_of_the_letters(*key),
                "p2's {} is back in player 1's half",
                key.name()
            );
        }
        for key in &p1 {
            assert!(
                !right_of_the_letters(*key),
                "p1's {} reaches into player 2's half",
                key.name()
            );
        }
    }

    /// Player 2 is not the lesser seat: a game that reads LT must work from
    /// both sides of the keyboard, so both blocks emit the same control set.
    #[test]
    fn both_seats_of_the_two_player_keyboard_reach_the_same_controls() {
        let template = find("keyboard-2p").unwrap();
        let controls = |player: u8| -> BTreeSet<Binding> {
            template
                .instantiate("Try", player)
                .unwrap()
                .entries
                .iter()
                .map(|(_, binding)| *binding)
                .collect()
        };
        assert_eq!(controls(1), controls(2));
    }

    #[test]
    fn the_two_player_keyboards_numpad_is_the_pad_face() {
        let p2 = instantiate("keyboard-2p", "P2", 2).unwrap();
        // The diamond in the pad's own geometry: 8 on top, 2 at the bottom.
        assert_eq!(
            keys_for(&p2, Binding::Button(XButton::Y)),
            vec![Key::Numpad8]
        );
        assert_eq!(
            keys_for(&p2, Binding::Button(XButton::A)),
            vec![Key::Numpad2]
        );
        assert_eq!(
            keys_for(&p2, Binding::Button(XButton::X)),
            vec![Key::Numpad4]
        );
        assert_eq!(
            keys_for(&p2, Binding::Button(XButton::B)),
            vec![Key::Numpad6]
        );
        assert_eq!(
            keys_for(
                &p2,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MAX
                }
            ),
            vec![Key::Up]
        );

        // Player 1's block is `keyboard-wasd`'s buttons, key for key — the
        // whole reason that claim is in the docs is that a player who learned
        // the solo template must not have to relearn anything to sit down at
        // player 1.
        let p1 = instantiate("keyboard-2p", "P1", 1).unwrap();
        let solo = instantiate("keyboard-wasd", "Solo", 1).unwrap();
        for control in [
            Binding::Button(XButton::A),
            Binding::Button(XButton::B),
            Binding::Button(XButton::X),
            Binding::Button(XButton::Y),
            Binding::Button(XButton::LeftBumper),
            Binding::Button(XButton::RightBumper),
            Binding::Trigger(Trigger::Left),
            Binding::Trigger(Trigger::Right),
            Binding::Button(XButton::LeftThumb),
            Binding::Button(XButton::RightThumb),
        ] {
            assert_eq!(
                keys_for(&p1, control),
                keys_for(&solo, control),
                "{control:?}"
            );
        }
    }

    /// `default` and `empty` are the built-ins, copied — same entries, a new
    /// name, and NOT protected any more (they are files now).
    #[test]
    fn the_builtin_seeds_copy_their_built_in() {
        let default = instantiate("default", "My layout", 1).unwrap();
        let builtin = Preset::builtin_default();
        assert_eq!(default.entries, builtin.entries);
        assert_eq!(default.name, "My layout");
        assert!(!default.protected);
        assert!(builtin.protected);

        let empty = instantiate("empty", "Blank", 1).unwrap();
        assert_eq!(empty.entries.len(), Preset::builtin_empty().entries.len());
        assert!(empty.entries.iter().all(|(k, _)| *k == Key::None));
    }
}
