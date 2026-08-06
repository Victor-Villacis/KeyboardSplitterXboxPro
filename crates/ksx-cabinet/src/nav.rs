//! **The navigation model: a joystick, two buttons, and nothing else.**
//!
//! The tiebreaker this whole surface was designed against — *can it be done
//! with a joystick, two buttons, no text entry, in ten seconds, by someone
//! standing at the cab with a game about to start?* — is a statement about
//! THIS module. Everything drawn elsewhere is a list of things you can already
//! do; this is the part that says what "doing" means.
//!
//! Four moves and two verbs:
//!
//! | input | meaning |
//! |---|---|
//! | up / down | previous / next item on this screen |
//! | left / right | previous / next SCREEN (the tab strip), or a horizontal choice inside one |
//! | confirm (A / South / Enter) | do the focused thing |
//! | back (B / East / Esc) | up one level; from the top level, nothing (never "quit") |
//!
//! Deliberately absent: no page-up, no home/end, no shoulder-button
//! accelerators, no long-press, no chord. Every one of those is a thing a
//! person has to be TOLD, and the population of this surface is "whoever is
//! standing at the cabinet", which includes the eight-year-old who just said
//! P3 does not work.
//!
//! # Pure on purpose
//!
//! Nothing here draws, reads a device, or knows what a screen contains beyond
//! how many rows it has. That is what lets the whole interaction model be
//! tested without a window — and it is the same reason `ksx setup`'s wizard is
//! a pure state machine with the observer supplied from outside.

/// One navigation input, whatever produced it — a pad, a keyboard, or a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
}

/// The screens, in the order the tab strip walks them.
///
/// Order is not arbitrary. [`Screen::ButtonCheck`] is first because it is why
/// the surface exists (docs/MAPPER-UX.md Build C); [`Screen::Status`] is
/// second because it is the other question asked at a cabinet ("is this
/// working?"); the three that CHANGE something come after the two that only
/// look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Screen {
    /// Press a button, see it light. The spine.
    ButtonCheck,
    /// Am I working: session, pads, drivers.
    Status,
    /// Start / stop emulation.
    Session,
    /// Pick a games.toml profile.
    Profiles,
    /// Pick a slot's preset.
    Presets,
}

impl Screen {
    /// Every screen, in tab order.
    pub const ALL: [Screen; 5] = [
        Screen::ButtonCheck,
        Screen::Status,
        Screen::Session,
        Screen::Profiles,
        Screen::Presets,
    ];

    /// The tab's label. Short enough to fit five across a 1280-wide panel at
    /// [`crate::theme::fs::MD`], and worded as the QUESTION each screen
    /// answers rather than as a noun — "Buttons", not "Diagnostics".
    pub fn label(self) -> &'static str {
        match self {
            Screen::ButtonCheck => "Buttons",
            Screen::Status => "Status",
            Screen::Session => "Start / Stop",
            Screen::Profiles => "Game",
            Screen::Presets => "Slots",
        }
    }

    /// The one line under the title: what this screen is FOR, in the words
    /// somebody standing at the cabinet would use.
    pub fn subtitle(self) -> &'static str {
        match self {
            Screen::ButtonCheck => {
                "Press a panel button. Left is what the panel sent, right is what the pad did."
            }
            Screen::Status => "Is ksx working right now?",
            Screen::Session => "Turn emulation on or off.",
            Screen::Profiles => "Which game profile the next start uses.",
            Screen::Presets => "Which preset each slot uses. Changing one restarts the session.",
        }
    }

    fn index(self) -> usize {
        Screen::ALL
            .iter()
            .position(|s| *s == self)
            .expect("every screen is in ALL")
    }

    fn step(self, delta: isize) -> Self {
        let len = Screen::ALL.len() as isize;
        let next = (self.index() as isize + delta).rem_euclid(len);
        Screen::ALL[next as usize]
    }
}

/// Where the user is: which screen, and which row of it.
///
/// One cursor, not one per screen. A cabinet user does not build a mental map
/// of five independent cursors; they expect the thing they just moved to be
/// where they left it, and moving to another screen to start at the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub screen: Screen,
    /// Row index on the current screen, always < `rows` (or 0 when `rows` is
    /// 0 — an empty screen still has a cursor, it just has nothing under it).
    row: usize,
    /// How many rows the CURRENT screen has. Set by the drawing code once per
    /// frame, before input is applied, because only the screen knows.
    rows: usize,
    /// A modal question is open; `Back` closes it and `Confirm` answers it.
    /// One level only — a cabinet surface that can be three dialogs deep has
    /// already failed the ten-second test.
    pub confirming: Option<Confirm>,
}

/// The one modal this surface has: "this will restart the session — yes or
/// no".
///
/// It exists for exactly one action (assigning a slot's preset), because that
/// is the only thing here that interrupts a running game. Start and Stop are
/// not confirmed: they are what the user came to do, they are instantly
/// visible, and they are one press to undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    /// The question, already worded by the caller.
    pub question: String,
    /// What happens if they say yes — spelled out, including the pad bounce.
    pub consequence: String,
    /// `true` = the cursor is on "Yes". Starts on **No**: a modal that opens
    /// with the destructive answer pre-selected turns a stray button press
    /// into an unplugged controller.
    pub yes: bool,
}

/// What one [`Nav`] did — the drawing code acts on this rather than reading
/// the focus back out, so "which key does what" lives in one place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Focus moved (or tried to); nothing else happened.
    Moved,
    /// Do the thing on row `row` of the current screen.
    Activate(usize),
    /// A modal was answered.
    Confirmed { yes: bool },
    /// `Back` at the top level: nothing to go back to.
    AtTop,
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            screen: Screen::ButtonCheck,
            row: 0,
            rows: 0,
            confirming: None,
        }
    }
}

impl Focus {
    /// Tell the cursor how big the current screen is. Called once per frame by
    /// the screen that is about to draw, before [`Self::apply`].
    ///
    /// Clamping here rather than at move time is what makes a list that
    /// SHRINKS (a profile deleted on disk, a session that ended) leave the
    /// cursor somewhere real instead of past the end.
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
        if self.row >= rows {
            self.row = rows.saturating_sub(1);
        }
    }

    /// The focused row. Meaningless when the screen has no rows, and callers
    /// check `rows` for that rather than being handed a fake index.
    pub fn row(&self) -> usize {
        self.row
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Is `row` the focused one? The predicate every drawn row asks.
    pub fn is_on(&self, row: usize) -> bool {
        self.confirming.is_none() && self.rows > 0 && self.row == row
    }

    /// Open the one modal.
    pub fn ask(&mut self, question: impl Into<String>, consequence: impl Into<String>) {
        self.confirming = Some(Confirm {
            question: question.into(),
            consequence: consequence.into(),
            yes: false,
        });
    }

    /// Apply one input.
    pub fn apply(&mut self, nav: Nav) -> Action {
        // A modal owns every input while it is open. Nothing behind it moves —
        // including the screen tabs, because a left-press meant for "No" must
        // never both answer the question and change the page.
        if let Some(confirm) = &mut self.confirming {
            return match nav {
                Nav::Left | Nav::Right | Nav::Up | Nav::Down => {
                    confirm.yes = !confirm.yes;
                    Action::Moved
                }
                Nav::Confirm => {
                    let yes = confirm.yes;
                    self.confirming = None;
                    Action::Confirmed { yes }
                }
                // Back is always No. A modal whose escape hatch did the thing
                // would be the worst possible arrangement of these two keys.
                Nav::Back => {
                    self.confirming = None;
                    Action::Confirmed { yes: false }
                }
            };
        }

        match nav {
            Nav::Up => {
                if self.rows > 0 {
                    // Wrapping, both ways. On a list of four slots, "up from
                    // the top" meaning "the bottom" saves three presses and
                    // costs nothing — there is no scrollbar to lose your place
                    // in.
                    self.row = (self.row + self.rows - 1) % self.rows;
                }
                Action::Moved
            }
            Nav::Down => {
                if self.rows > 0 {
                    self.row = (self.row + 1) % self.rows;
                }
                Action::Moved
            }
            Nav::Left => {
                self.screen = self.screen.step(-1);
                self.row = 0;
                Action::Moved
            }
            Nav::Right => {
                self.screen = self.screen.step(1);
                self.row = 0;
                Action::Moved
            }
            Nav::Confirm => {
                if self.rows == 0 {
                    Action::Moved
                } else {
                    Action::Activate(self.row)
                }
            }
            // Back from a row goes to the top of the screen; back from the top
            // of the screen goes nowhere. It is NEVER quit: the window is
            // closed by the window's own close button, and the DAEMON is
            // quitted from the tray. A back button that eventually kills
            // emulation is a back button nobody presses twice.
            Nav::Back => {
                if self.row > 0 {
                    self.row = 0;
                    Action::Moved
                } else {
                    Action::AtTop
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(screen: Screen, rows: usize) -> Focus {
        let mut focus = Focus {
            screen,
            ..Focus::default()
        };
        focus.set_rows(rows);
        focus
    }

    #[test]
    fn up_and_down_wrap_so_a_four_row_list_is_never_more_than_two_presses_away() {
        let mut focus = on(Screen::Presets, 4);
        assert_eq!(focus.row(), 0);
        focus.apply(Nav::Up);
        assert_eq!(focus.row(), 3, "up from the top is the bottom");
        focus.apply(Nav::Down);
        assert_eq!(focus.row(), 0);
        for _ in 0..3 {
            focus.apply(Nav::Down);
        }
        assert_eq!(focus.row(), 3);
    }

    #[test]
    fn left_and_right_walk_the_screens_and_reset_the_row() {
        let mut focus = on(Screen::ButtonCheck, 4);
        focus.apply(Nav::Down);
        assert_eq!(focus.row(), 1);
        focus.apply(Nav::Right);
        assert_eq!(focus.screen, Screen::Status);
        assert_eq!(focus.row(), 0, "a new screen starts at the top");
        focus.apply(Nav::Left);
        assert_eq!(focus.screen, Screen::ButtonCheck);
        // ...and the strip wraps, so five screens are never more than two
        // presses apart.
        focus.apply(Nav::Left);
        assert_eq!(focus.screen, Screen::Presets);
    }

    #[test]
    fn confirm_activates_the_focused_row() {
        let mut focus = on(Screen::Profiles, 3);
        focus.apply(Nav::Down);
        assert_eq!(focus.apply(Nav::Confirm), Action::Activate(1));
    }

    /// An empty screen still has a cursor; it just has nothing to activate,
    /// and pressing confirm on it does nothing rather than firing row 0 of a
    /// list that is not there.
    #[test]
    fn a_screen_with_no_rows_cannot_be_activated() {
        let mut focus = on(Screen::Profiles, 0);
        assert_eq!(focus.apply(Nav::Confirm), Action::Moved);
        assert!(!focus.is_on(0));
    }

    /// The list shrank underneath the cursor (a profile removed on disk).
    #[test]
    fn shrinking_a_list_leaves_the_cursor_somewhere_real() {
        let mut focus = on(Screen::Profiles, 5);
        for _ in 0..4 {
            focus.apply(Nav::Down);
        }
        assert_eq!(focus.row(), 4);
        focus.set_rows(2);
        assert_eq!(focus.row(), 1);
        assert!(focus.is_on(1));
    }

    #[test]
    fn back_goes_to_the_top_of_the_screen_and_then_stops() {
        let mut focus = on(Screen::Presets, 4);
        focus.apply(Nav::Down);
        focus.apply(Nav::Down);
        assert_eq!(focus.apply(Nav::Back), Action::Moved);
        assert_eq!(focus.row(), 0);
        // From the top, Back does nothing — it is never "quit".
        assert_eq!(focus.apply(Nav::Back), Action::AtTop);
        assert_eq!(focus.screen, Screen::Presets, "and never changes screen");
    }

    /// **The modal starts on No.** A cabinet gets stray presses — a player
    /// leaning on the panel, a kid mashing — and the one action behind this
    /// modal unplugs four controllers.
    #[test]
    fn the_confirm_dialog_starts_on_no_and_back_means_no() {
        let mut focus = on(Screen::Presets, 4);
        focus.ask("Use \"IPAC P2\" for slot 1?", "the pads will replug");
        assert_eq!(focus.confirming.as_ref().map(|c| c.yes), Some(false));
        assert_eq!(focus.apply(Nav::Back), Action::Confirmed { yes: false });
        assert!(focus.confirming.is_none());

        // Confirming outright, from No, needs a deliberate move first.
        focus.ask("q", "c");
        assert_eq!(focus.apply(Nav::Confirm), Action::Confirmed { yes: false });
        focus.ask("q", "c");
        focus.apply(Nav::Right);
        assert_eq!(focus.apply(Nav::Confirm), Action::Confirmed { yes: true });
    }

    /// While the modal is up, nothing behind it moves — a left-press aimed at
    /// "No" must not also change the page.
    #[test]
    fn a_modal_swallows_every_input_including_the_screen_tabs() {
        let mut focus = on(Screen::Presets, 4);
        focus.apply(Nav::Down);
        focus.ask("q", "c");
        focus.apply(Nav::Left);
        focus.apply(Nav::Up);
        assert_eq!(focus.screen, Screen::Presets);
        assert_eq!(focus.row(), 1);
        assert!(
            !focus.is_on(1),
            "the row is not focused while a modal is up"
        );
    }

    /// Every screen has a label and a subtitle, and none of them is a noun
    /// nobody at a cabinet would say.
    #[test]
    fn every_screen_says_what_it_is_for() {
        for screen in Screen::ALL {
            assert!(!screen.label().is_empty());
            assert!(screen.subtitle().len() > 12, "{:?}", screen);
        }
        assert_eq!(Screen::ALL.len(), 5);
    }
}
