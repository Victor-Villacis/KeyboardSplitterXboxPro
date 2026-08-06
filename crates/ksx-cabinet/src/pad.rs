//! **Driving this window from the arcade panel — the hard requirement.**
//!
//! A cabinet has no mouse and no keyboard, so a config surface that can only
//! be driven by one is a surface nobody at the cabinet can use. There are two
//! input paths and **which one works depends on whether emulation is
//! running**, which is the finding this module exists for:
//!
//! | emulation | what the panel is | what drives this window |
//! |---|---|---|
//! | **stopped** | an ordinary keyboard (Interception), or typethrough re-injecting its keys (WinUSB) | **winit**, exactly like any desktop app — egui sees real key events |
//! | **running** | captured below win32k, and on WinUSB deliberately muted from typethrough (`daemon::control_loop_with` calls `set_emulating(true)` before the session exists) | **nothing, via winit.** The window is on a machine whose panel produces no keystrokes at all |
//!
//! That second row is not a limitation to work around; it is the whole point
//! of ksx. The panel's presses are being translated into virtual pad state,
//! and those pads are **real XInput devices on this machine** — so the way to
//! read the panel while emulation is running is to read what ksx just
//! published. `XInputGetState` against our own pads, in the same process that
//! created them.
//!
//! The consequence is worth stating plainly: **while a session is running, a
//! press does both things.** It moves this cursor and it plays the game. That
//! is why [`PadNav::poll`] is only ever called with the cabinet window
//! FOCUSED — a window nobody is looking at must not eat a fireball motion.
//!
//! # Why not gilrs
//!
//! Five constants, one struct and one function against the `windows-sys` this
//! workspace already links, versus a crate with its own event loop, its own
//! device thread and its own opinion about hotplug. `daemon/tray.rs` made the
//! same call for the same reason, and wrote down the same rationale.
//!
//! # Cost
//!
//! `XInputGetState` on a CONNECTED pad is a cheap user-mode call. On an absent
//! user index it is documented to be slow (it goes looking), so absent indices
//! are probed at most every [`PROBE_ABSENT`] rather than every frame — the
//! difference between a 60 Hz UI and a 60 Hz UI that stutters on a cabinet
//! with two players plugged in.

use std::time::{Duration, Instant};

use crate::nav::Nav;

/// How often an ABSENT XInput index is re-probed. Long enough that the slow
/// path costs nothing; short enough that plugging a pad feels immediate.
const PROBE_ABSENT: Duration = Duration::from_millis(1500);

/// Menu autorepeat, the standard two-stage feel: a deliberate first move, then
/// a comfortable run. Tuned so that holding up through a 12-row preset list
/// takes about a second and a half — fast enough not to be a chore, slow
/// enough that nobody overshoots by three.
const REPEAT_DELAY: Duration = Duration::from_millis(420);
const REPEAT_RATE: Duration = Duration::from_millis(110);

/// Stick deflection that counts as a direction — Microsoft's own documented
/// left-thumb deadzone. Using theirs rather than inventing one means a stick
/// that a game considers centred is centred here too.
const THUMB_DEADZONE: i16 = 7849;

/// The four XInput user indices — the whole of XInput, and therefore the whole
/// of what can drive this window while a session is running.
///
/// **The limit is the PERSONA, not the slot number.** A `playstation` persona
/// is a DS4 on plain HID and has no XInput index at all (`ksx_core::Persona`),
/// so a cabinet whose slots are all PlayStation — an entirely ordinary
/// MAME/RetroArch cabinet, since those read DS4 and XInput-only titles do not
/// — presents ZERO XInput pads. With emulation running such a panel produces
/// no keystrokes either, and this window then cannot be driven from the
/// cabinet at all. That is a real hole and it is stated on screen rather than
/// left to be discovered: `screens::footer` says so in the one case it is
/// true, in `WARN`, instead of the old "panel: keyboard only" — which was the
/// opposite of the truth in exactly that state.
pub const MAX_PADS: u32 = 4;

/// One frame's worth of pad state, reduced to the six things this UI knows
/// about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Reading {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    confirm: bool,
    back: bool,
}

impl Reading {
    /// The six as an array, paired with what each one means, so edge detection
    /// and autorepeat are written once instead of six times.
    fn pressed(self) -> [(Nav, bool); 6] {
        [
            (Nav::Up, self.up),
            (Nav::Down, self.down),
            (Nav::Left, self.left),
            (Nav::Right, self.right),
            (Nav::Confirm, self.confirm),
            (Nav::Back, self.back),
        ]
    }
}

/// Reads OUR OWN virtual pads and turns them into [`Nav`]s.
///
/// One instance per window. Holds only edge-detection state; it opens nothing
/// and owns no thread.
pub struct PadNav {
    /// Previous MERGED reading, for edge detection. **One**, not one per
    /// index: every pad's inputs are OR'd into a single cursor state (see
    /// [`Self::poll`]), so there is one thing to detect an edge on. The first
    /// pass held an array of four and only ever wrote `[0]`, which read as if
    /// per-pad edges existed.
    last: Reading,
    /// Which indices answered last time we looked.
    connected: [bool; MAX_PADS as usize],
    /// When an absent index may be probed again.
    probe_after: Instant,
    /// The direction currently held, and when it should fire next. Directions
    /// autorepeat; the two BUTTONS never do — a confirm that repeats would
    /// walk a list of profiles and start one.
    held: Option<(Nav, Instant)>,
    /// Any pad answered at all, ever. Drives the honest "no pads are plugged"
    /// line rather than a panel that silently does nothing.
    seen_any: bool,
}

impl Default for PadNav {
    fn default() -> Self {
        Self::new()
    }
}

impl PadNav {
    pub fn new() -> Self {
        Self {
            last: Reading::default(),
            connected: [false; MAX_PADS as usize],
            probe_after: Instant::now(),
            held: None,
            seen_any: false,
        }
    }

    /// Have we ever heard from a pad? A cabinet whose panel does nothing here
    /// deserves the sentence, not the silence.
    pub fn any_pad_seen(&self) -> bool {
        self.seen_any
    }

    /// How many pads answered on the last poll.
    pub fn connected_count(&self) -> usize {
        self.connected.iter().filter(|c| **c).count()
    }

    /// One frame. Returns every navigation the panel produced since the last
    /// call — usually none, at most a couple.
    ///
    /// `now` is passed in rather than read so the autorepeat is testable
    /// without sleeping, the same reason the macro scheduler takes its clock
    /// (docs/INPUT-TRANSFORMS.md §1c).
    pub fn poll(&mut self, now: Instant) -> Vec<Nav> {
        let probe_absent = now >= self.probe_after;
        if probe_absent {
            self.probe_after = now + PROBE_ABSENT;
        }

        // Every pad's inputs are OR'd together on purpose. On a four-player
        // cabinet the person fixing P3 is standing at P3's stick, and making
        // them walk to P1 to drive the menu would be exactly the kind of
        // detail that makes a surface unusable in the room it was built for.
        let mut merged = Reading::default();
        for index in 0..MAX_PADS as usize {
            if !self.connected[index] && !probe_absent {
                continue;
            }
            match read_pad(index as u32) {
                Some(reading) => {
                    self.connected[index] = true;
                    self.seen_any = true;
                    merged.up |= reading.up;
                    merged.down |= reading.down;
                    merged.left |= reading.left;
                    merged.right |= reading.right;
                    merged.confirm |= reading.confirm;
                    merged.back |= reading.back;
                }
                None => self.connected[index] = false,
            }
        }
        let previous = self.last;
        self.last = merged;
        self.edges(previous, merged, now)
    }

    /// Edge detection plus autorepeat, factored out so it can be driven by a
    /// test with no XInput anywhere near it.
    fn edges(&mut self, previous: Reading, now_state: Reading, now: Instant) -> Vec<Nav> {
        let mut out = Vec::new();
        for ((nav, is_down), (_, was_down)) in now_state.pressed().iter().zip(previous.pressed()) {
            if *is_down && !was_down {
                out.push(*nav);
                // Only the four directions repeat. Confirm and Back are
                // one-shot, always: an autorepeating confirm on a list of game
                // profiles would launch one, and then another.
                if matches!(nav, Nav::Up | Nav::Down | Nav::Left | Nav::Right) {
                    self.held = Some((*nav, now + REPEAT_DELAY));
                }
            }
        }
        // The held direction was released (or another one took over).
        if let Some((held, _)) = self.held {
            let still_down = now_state
                .pressed()
                .iter()
                .any(|(nav, down)| *nav == held && *down);
            if !still_down {
                self.held = None;
            }
        }
        if let Some((held, due)) = &mut self.held {
            if now >= *due {
                out.push(*held);
                *due = now + REPEAT_RATE;
            }
        }
        out
    }
}

/// Read one XInput user index. `None` = nothing is plugged into it.
#[cfg(windows)]
fn read_pad(index: u32) -> Option<Reading> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN,
        XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_STATE,
    };

    // SAFETY: `state` is a plain C struct; zero is a valid starting value and
    // XInputGetState fills it in on success. The call takes a scalar index and
    // an out-pointer to storage we own, and cannot fail unsafely.
    let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
    let ok = unsafe { XInputGetState(index, &mut state) };
    if ok != ERROR_SUCCESS {
        // ERROR_DEVICE_NOT_CONNECTED, or anything else: either way there is no
        // pad here to read, and pretending otherwise would strand the cursor.
        return None;
    }
    let pad = state.Gamepad;
    let held = |bit: u16| pad.wButtons & bit != 0;
    Some(Reading {
        // Stick OR dpad, both ways. An arcade stick maps to one or the other
        // depending on the persona and the preset, and a person pushing the
        // stick up does not care which.
        up: held(XINPUT_GAMEPAD_DPAD_UP) || pad.sThumbLY > THUMB_DEADZONE,
        down: held(XINPUT_GAMEPAD_DPAD_DOWN) || pad.sThumbLY < -THUMB_DEADZONE,
        left: held(XINPUT_GAMEPAD_DPAD_LEFT) || pad.sThumbLX < -THUMB_DEADZONE,
        right: held(XINPUT_GAMEPAD_DPAD_RIGHT) || pad.sThumbLX > THUMB_DEADZONE,
        // The two buttons, in the position every console has taught: the
        // SOUTH face button confirms, the EAST one goes back. Named by
        // position rather than by letter because that is what the panel is
        // labelled with (`ksx setup` prompts the same way — "SOUTH", not "A").
        confirm: held(XINPUT_GAMEPAD_A),
        back: held(XINPUT_GAMEPAD_B),
    })
}

#[cfg(not(windows))]
fn read_pad(_index: u32) -> Option<Reading> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(nav: Nav) -> Reading {
        let mut reading = Reading::default();
        match nav {
            Nav::Up => reading.up = true,
            Nav::Down => reading.down = true,
            Nav::Left => reading.left = true,
            Nav::Right => reading.right = true,
            Nav::Confirm => reading.confirm = true,
            Nav::Back => reading.back = true,
        }
        reading
    }

    /// One press is one move, however many frames the button is held for.
    #[test]
    fn holding_a_button_does_not_produce_a_move_per_frame() {
        let mut pads = PadNav::new();
        let t0 = Instant::now();
        let down = press(Nav::Down);
        assert_eq!(pads.edges(Reading::default(), down, t0), [Nav::Down]);
        // Same state, one frame later: nothing new.
        assert!(pads
            .edges(down, down, t0 + Duration::from_millis(16))
            .is_empty());
    }

    /// ...until the autorepeat delay is up, and then at the repeat rate.
    #[test]
    fn a_held_direction_repeats_after_the_delay() {
        let mut pads = PadNav::new();
        let t0 = Instant::now();
        let down = press(Nav::Down);
        pads.edges(Reading::default(), down, t0);
        assert!(pads
            .edges(down, down, t0 + REPEAT_DELAY - Duration::from_millis(1))
            .is_empty());
        assert_eq!(pads.edges(down, down, t0 + REPEAT_DELAY), [Nav::Down]);
        assert_eq!(
            pads.edges(down, down, t0 + REPEAT_DELAY + REPEAT_RATE),
            [Nav::Down]
        );
    }

    /// **Confirm never repeats.** A held A on a list of game profiles would
    /// otherwise start one, and then start the next.
    #[test]
    fn confirm_and_back_never_autorepeat_however_long_they_are_held() {
        let mut pads = PadNav::new();
        let t0 = Instant::now();
        let a = press(Nav::Confirm);
        assert_eq!(pads.edges(Reading::default(), a, t0), [Nav::Confirm]);
        for ms in [100, 500, 2_000, 10_000] {
            assert!(
                pads.edges(a, a, t0 + Duration::from_millis(ms)).is_empty(),
                "confirm repeated after {ms} ms"
            );
        }
    }

    /// Releasing the stick stops the repeat, even mid-run.
    #[test]
    fn releasing_a_direction_stops_the_repeat() {
        let mut pads = PadNav::new();
        let t0 = Instant::now();
        let up = press(Nav::Up);
        pads.edges(Reading::default(), up, t0);
        pads.edges(up, up, t0 + REPEAT_DELAY);
        assert!(pads
            .edges(up, Reading::default(), t0 + REPEAT_DELAY + REPEAT_RATE)
            .is_empty());
        assert!(pads.held.is_none());
    }

    /// A diagonal is two presses, not one — which is what makes "left one
    /// screen and up one row" work as a single stick motion.
    #[test]
    fn a_diagonal_produces_both_directions() {
        let mut pads = PadNav::new();
        let diagonal = Reading {
            up: true,
            left: true,
            ..Reading::default()
        };
        let moves = pads.edges(Reading::default(), diagonal, Instant::now());
        assert!(moves.contains(&Nav::Up) && moves.contains(&Nav::Left));
    }

    /// On a machine with no pads at all, polling is silent and says so —
    /// never a cursor that appears to work and does not.
    #[test]
    fn with_no_pads_connected_nothing_is_produced_and_the_fact_is_reportable() {
        let mut pads = PadNav::new();
        // On CI (and on Linux) `read_pad` answers None for every index.
        let moves = pads.poll(Instant::now());
        if !pads.any_pad_seen() {
            assert!(moves.is_empty());
            assert_eq!(pads.connected_count(), 0);
        }
    }
}
