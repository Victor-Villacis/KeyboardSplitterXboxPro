//! **A list longer than the panel, walked by a joystick.**
//!
//! [`ksx_api::MAX_SLOTS`] is 16. A row on this surface is 72 px tall by rule
//! ([`crate::theme::ctl::ROW`]) and it stays 72 px: shrinking rows until
//! sixteen fit is the one fix this surface is not allowed to make, because the
//! size *is* the feature — a cabinet is read standing, from six feet, by
//! somebody who is not going to lean in. Sixteen rows are therefore taller
//! than any cabinet panel, and something has to decide which of them are on
//! screen.
//!
//! # That something is not a scroll bar
//!
//! `screens::body` has had an [`egui::ScrollArea`] around it since the first
//! pass, and it never once helped. A scroll bar is a *mouse* affordance: it is
//! turned by a wheel, dragged by a pointer, or jumped by a keyboard's Page
//! keys, and docs/SURFACES.md §4 says the population of this surface has none
//! of the three — **at an arcade cabinet there is no mouse and no keyboard,
//! the panel is the input.** So when `MAX_SLOTS` went from 8 to 16, the slot
//! list did the worst available thing: it drew all sixteen rows, let the
//! cursor walk to row 7, let Ⓐ activate row 7, and did not put row 7 on the
//! screen. The player whose slot is row 7 could not see their own controls,
//! and nothing anywhere said the list continued.
//!
//! A list that stops at four is a bug. A list that stops at four *and lies
//! about it* is this one.
//!
//! # So the panel pages
//!
//! Two pure functions, no state, no scroll offset, no animation:
//!
//! - [`per_page`] — how many whole rows fit in the height the screen has left;
//! - [`window`] — which slice of the list is on screen, given where the cursor
//!   is.
//!
//! Pure for the same reason [`crate::nav`] is pure: the whole interaction
//! model of this surface is testable without a window, and "can a person reach
//! row 15" is a question that should be answered by an assertion rather than
//! by somebody standing at a cabinet counting.
//!
//! # Pages, not a following cursor
//!
//! [`window`] anchors to a page (`focus / per_page`) rather than centring the
//! cursor. The difference is what MOVES when the joystick moves:
//!
//! | | pressing down does |
//! |---|---|
//! | cursor-anchored | the cursor stays put, **the list slides under it** |
//! | page-anchored | the cursor walks down a **still list**, and the page flips when it runs out |
//!
//! The second is what an appliance does, and it is the one that survives being
//! read from six feet: a list that creeps by one row on every press gives the
//! eye nothing to hold on to. It also costs no extra input, which
//! [`crate::nav`] is strict about — there is no Page key here, because there is
//! no key.
//!
//! The one exception is the end of the list, where an anchored page would be a
//! stub — three rows of a seven-row page, and four blank slots below them
//! reading as "this cabinet has nothing there". The last page is pulled up to
//! be a whole page instead, so the final press shifts the list by a row or two.
//! That is one small movement at one end, in exchange for never drawing a
//! ragged screen.

use std::ops::Range;

/// The most rows this module will claim fit at once.
///
/// Not a design limit — it is a guard on the `f32 -> usize` conversion below,
/// so that a nonsense height (an off-screen viewport, a panel measured before
/// layout settled) can never turn into an allocation the size of a screen.
const CEILING: f32 = 1024.0;

/// How many whole rows of `pitch` pixels fit in `available` pixels.
///
/// `pitch` is a full row **plus its trailing gap** — see `screens::pitch`,
/// which has to add the `item_spacing` egui inserts between two allocated
/// widgets whether anybody asked for it or not.
///
/// Deliberately *not* the "n rows need n-1 gaps" arithmetic. `n * pitch`
/// over-counts the screen by exactly one trailing gap, and that gap of slack is
/// the difference between a last row that is whole and a last row that is
/// shaved — which on a focus plate with a 5 px ring outside it (`theme`'s three
/// redundant focus signals) is the difference between a visible cursor and a
/// clipped one.
pub fn per_page(available: f32, pitch: f32) -> usize {
    if !available.is_finite() || !pitch.is_finite() || pitch <= 0.0 || available < pitch {
        // One, never zero. A page with no rows on it is a screen with no
        // cursor on it, and this surface's cursor is its entire interaction
        // model. One row that overflows is still one row somebody can see.
        return 1;
    }
    (available / pitch).floor().min(CEILING) as usize
}

/// The slice of a `count`-row list that is on screen with the cursor on
/// `focus`.
///
/// **The invariant, and the only one that matters: the returned range always
/// contains `focus`.** Every row of every list on this surface is therefore
/// reachable by the joystick alone, which is the whole requirement
/// (docs/SURFACES.md §4).
///
/// `focus` past the end is clamped rather than refused: `nav::Focus::set_rows`
/// already clamps the cursor when a list shrinks, and a second opinion here
/// costs one `min` and removes a panic from a paint path.
pub fn window(count: usize, focus: usize, per_page: usize) -> Range<usize> {
    if count == 0 {
        return 0..0;
    }
    let per_page = per_page.clamp(1, count);
    if per_page == count {
        return 0..count;
    }
    let focus = focus.min(count - 1);
    // Page-anchored: the cursor walks a still list, and the page flips under
    // it when it runs out (see the module docs).
    let first = (focus / per_page) * per_page;
    // ...except at the end, where the anchored page would be a stub. Pull it
    // up so the last page is a whole page.
    let first = first.min(count - per_page);
    first..first + per_page
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel this surface was designed against, and the row it draws on
    /// it. A 1080p cabinet screen minus the head dock, the foot dock and the
    /// legend leaves something like this for the list itself.
    const PANEL: f32 = 640.0;
    /// `theme::ctl::ROW` (72) + the caller's gap (16) + egui's `item_spacing`
    /// (16). Named here rather than imported so this module stays free of
    /// egui.
    const PITCH: f32 = 104.0;

    /// **The bug, as a property.**
    ///
    /// `MAX_SLOTS` was lifted to 16 and the slot list kept drawing four. Row 7
    /// existed, was focusable and was off the bottom of the panel — so a
    /// player on slot 7 could not see their own controls.
    ///
    /// Whatever the panel's height turns out to be on a given cabinet, every
    /// slot has to be on screen when the cursor is on it. Asserted for every
    /// page size a real panel could produce, not just the one this machine has.
    #[test]
    fn every_one_of_sixteen_slots_is_on_screen_when_the_cursor_is_on_it() {
        for per_page in 1..=16 {
            for focus in 0..16 {
                let window = window(16, focus, per_page);
                assert!(
                    window.contains(&focus),
                    "slot {} is unreachable: with {per_page} row(s) on screen the panel shows \
                     {window:?}",
                    focus + 1
                );
            }
        }
    }

    /// The same property, for every list length any screen here can produce —
    /// profiles, presets on disk, slots — against every page size.
    #[test]
    fn the_window_always_contains_the_cursor() {
        for count in 0..40_usize {
            for per_page in 1..=40 {
                for focus in 0..count.max(1) {
                    let window = window(count, focus, per_page);
                    if count == 0 {
                        assert!(window.is_empty(), "an empty list has an empty window");
                        continue;
                    }
                    assert!(
                        window.contains(&focus),
                        "count {count}, per_page {per_page}, focus {focus} -> {window:?}"
                    );
                    assert!(
                        window.end <= count,
                        "the window ran past the list: {window:?}"
                    );
                    assert_eq!(
                        window.len(),
                        per_page.min(count),
                        "the window is not a full page: {window:?}"
                    );
                }
            }
        }
    }

    /// A list that fits is not paged at all: no hints, no arithmetic, the
    /// same screen it always was. Four slots on a cabinet panel must not
    /// suddenly grow a "0 more below".
    #[test]
    fn a_list_that_fits_is_never_paged() {
        assert_eq!(window(4, 0, per_page(PANEL, PITCH)), 0..4);
        assert_eq!(window(4, 3, per_page(PANEL, PITCH)), 0..4);
        assert_eq!(window(6, 5, 6), 0..6);
        assert_eq!(window(6, 5, 9), 0..6);
    }

    /// The last page is a whole page, not the two rows left over — four blank
    /// slots under a stub read as "this cabinet has nothing there".
    #[test]
    fn the_last_page_is_a_whole_page() {
        // 16 slots, 6 to a page: pages 0..6, 6..12, then 10..16 rather than
        // the 12..16 stub an unclamped page would give.
        assert_eq!(window(16, 0, 6), 0..6);
        assert_eq!(window(16, 6, 6), 6..12);
        assert_eq!(window(16, 15, 6), 10..16);
        assert_eq!(window(16, 12, 6), 10..16);
    }

    /// The cursor walks a still list; the page flips only when it runs out.
    /// Pressing down through a page must not slide the list by one every time.
    #[test]
    fn the_page_holds_still_while_the_cursor_crosses_it() {
        let page: Vec<Range<usize>> = (0..4).map(|focus| window(16, focus, 4)).collect();
        assert!(
            page.windows(2).all(|pair| pair[0] == pair[1]),
            "the list moved under the cursor within one page: {page:?}"
        );
        assert_eq!(window(16, 4, 4), 4..8, "and flips when the cursor leaves");
    }

    /// A page of zero rows would be a screen with no cursor on it — this
    /// surface's cursor is its whole interaction model, so one row is the
    /// floor no matter how absurd the measurement.
    #[test]
    fn a_page_is_never_empty_however_the_panel_measures() {
        for available in [f32::NAN, f32::INFINITY, -1.0, 0.0, 1.0, 103.9] {
            assert_eq!(per_page(available, PITCH), 1, "available {available}");
        }
        for pitch in [f32::NAN, f32::INFINITY, 0.0, -72.0] {
            assert_eq!(per_page(PANEL, pitch), 1, "pitch {pitch}");
        }
        assert_eq!(
            window(16, 9, 0),
            9..10,
            "a zero page still shows the cursor"
        );
    }

    /// ...and never a page bigger than a screen, whatever a stale layout
    /// reports for the panel's height.
    #[test]
    fn a_page_is_never_bigger_than_a_screen() {
        assert_eq!(per_page(f32::MAX, PITCH), CEILING as usize);
        assert_eq!(per_page(PANEL, PITCH), 6);
    }

    /// The cursor past the end of the list is clamped, not panicked on —
    /// paint paths do not get to unwrap.
    #[test]
    fn a_cursor_past_the_end_lands_on_the_last_page() {
        assert_eq!(window(16, 99, 4), 12..16);
        assert_eq!(window(1, 99, 4), 0..1);
        assert_eq!(window(0, 99, 4), 0..0);
    }
}
