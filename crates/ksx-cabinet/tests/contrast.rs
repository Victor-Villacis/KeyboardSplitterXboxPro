//! **The contrast contract for the 10-foot surface.**
//!
//! The twin of `ksx-studio/tests/contrast.rs`. That one parses `studio.css`,
//! because on the web the stylesheet is the source of truth; this one reads
//! [`theme::role`] directly, because here the Rust constants *are* the theme.
//! Neither imports the other: twelve lines of duplicated WCAG arithmetic is a
//! smaller price than a crate dependency between two UIs that share nothing
//! else, and the floors differ anyway (this surface has one theme, no light
//! mode, and tints twice as hard).
//!
//! # Why the composed pairs matter more here
//!
//! A cabinet screen is read standing, from six feet, in a dim room. The pairs
//! that fail are never the obvious ones — `TEXT` on `SURFACE` was never in
//! doubt. They are the ones nobody writes down:
//!
//! - the focus plate (`ACCENT_ON` on `ACCENT`), which is **the cursor**: if
//!   that pair softens, the surface loses its only pointer;
//! - a state colour on its own `tint()`, which is what a banner and a pill
//!   actually are — and where `#f4675f` (the web's `--danger`) measures
//!   4.44:1 on `INSET`, which is why this surface keeps a lighter red;
//! - `ACCENT_ON` on `OK`, which only exists because a fully-lit pad key
//!   reuses the accent's on-colour against a green plate.
//!
//! Every one of those is invisible in a list of constants and obvious in a
//! table of ratios, which is what this test prints.

use ksx_cabinet::theme::{role, FOCUS_STROKE};

const TEXT_FLOOR: f64 = 4.5;
const NON_TEXT_FLOOR: f64 = 3.0;
/// Same calibrated separator floor as the web surface: the faintest hairline
/// the reviewed theme already shipped. Not a WCAG number — the perceivable
/// boundary here is the focus plate, held to `NON_TEXT_FLOOR` below.
const SEPARATOR_FLOOR: f64 = 1.12;

fn luminance(c: egui::Color32) -> f64 {
    fn chan(v: u8) -> f64 {
        let s = v as f64 / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * chan(c.r()) + 0.7152 * chan(c.g()) + 0.0722 * chan(c.b())
}

fn ratio(fg: egui::Color32, bg: egui::Color32) -> f64 {
    let (a, b) = (luminance(fg), luminance(bg));
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// Composite an `Color32` with unmultiplied alpha over an opaque ground —
/// what `role::tint` produces and what the painter actually blends.
fn over(fg: egui::Color32, bg: egui::Color32) -> egui::Color32 {
    let a = fg.a() as f64 / 255.0;
    let mix = |f: u8, b: u8| (f as f64 * a + b as f64 * (1.0 - a)).round() as u8;
    egui::Color32::from_rgb(
        mix(fg.r(), bg.r()),
        mix(fg.g(), bg.g()),
        mix(fg.b(), bg.b()),
    )
}

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, floor: f64, label: &str, fg: egui::Color32, bg: egui::Color32) {
        let v = ratio(fg, bg);
        self.lines
            .push(format!("  {v:6.2}  (>={floor:.2})  {label}"));
        if v < floor {
            self.failures
                .push(format!("{label} = {v:.2}:1, floor {floor:.2}:1"));
        }
    }

    fn finish(self, what: &str) {
        println!("── cabinet: {what} ──");
        for l in &self.lines {
            println!("{l}");
        }
        assert!(
            self.failures.is_empty(),
            "\n{} contrast failure(s) in cabinet {what}:\n  {}\n\n\
             Fix the ROLE in theme.rs, not the screen that revealed it.\n",
            self.failures.len(),
            self.failures.join("\n  ")
        );
    }
}

/// The three grounds a colour can land on: the page, a card, a lifted row.
fn grounds() -> [(&'static str, egui::Color32); 3] {
    [
        ("SURFACE", role::SURFACE),
        ("RAISED", role::RAISED),
        ("INSET", role::INSET),
    ]
}

#[test]
fn text_tiers_clear_the_floor() {
    let mut r = Report::default();
    for (gn, g) in grounds() {
        for (tn, t) in [
            ("TEXT", role::TEXT),
            ("TEXT_2", role::TEXT_2),
            ("TEXT_3", role::TEXT_3),
        ] {
            r.check(TEXT_FLOOR, &format!("{tn} on {gn}"), t, g);
        }
    }
    r.finish("text tiers");
}

#[test]
fn coloured_roles_clear_the_floor_as_text() {
    let mut r = Report::default();
    for (gn, g) in grounds() {
        for (rn, c) in [
            ("ACCENT", role::ACCENT),
            ("COOL", role::COOL),
            ("OK", role::OK),
            ("WARN", role::WARN),
            ("DANGER", role::DANGER),
        ] {
            r.check(TEXT_FLOOR, &format!("{rn} on {gn}"), c, g);
        }
    }
    r.finish("coloured roles as text");
}

/// `screens::flash` fills at `tint(colour, 38)` and writes the same colour on
/// top; `screens::pill` does it at `tint(colour, 40)`. Both are the state
/// triad from DESIGN-SYSTEM §3, and both are drawn over any of the grounds.
#[test]
fn state_text_clears_the_floor_on_its_own_tint() {
    let mut r = Report::default();
    for (gn, g) in grounds() {
        for (rn, c) in [
            ("OK", role::OK),
            ("WARN", role::WARN),
            ("DANGER", role::DANGER),
        ] {
            for alpha in [38u8, 40u8] {
                let plate = over(role::tint(c, alpha), g);
                r.check(
                    TEXT_FLOOR,
                    &format!("{rn} on tint({rn},{alpha}) over {gn}"),
                    c,
                    plate,
                );
            }
        }
    }
    r.finish("state text on its own tint");
}

/// **The cursor.** `screens::row` fills the focused row with `ACCENT` and
/// writes `ACCENT_ON` on it, then strokes `ACCENT` around it. If any of these
/// three soften, the surface loses the only thing telling a player where they
/// are — see the `FOCUS_STROKE` note in `theme.rs`.
#[test]
fn the_focus_plate_is_unmistakable() {
    let mut r = Report::default();
    r.check(
        TEXT_FLOOR,
        "ACCENT_ON on ACCENT (focused row label)",
        role::ACCENT_ON,
        role::ACCENT,
    );
    for (gn, g) in grounds() {
        r.check(
            NON_TEXT_FLOOR,
            &format!("ACCENT focus stroke on {gn}"),
            role::ACCENT,
            g,
        );
    }
    r.finish("focus plate");

    // Bound to a local first: `assert!` straight on a `const` is
    // `clippy::assertions_on_constants`, and the point here is a guard on the
    // VALUE somebody may edit in theme.rs, not a claim about a literal.
    let stroke = FOCUS_STROKE;
    assert!(
        stroke >= 4.0,
        "the focus stroke is the cursor on a surface with no pointer; \
         {stroke}px is too thin to find from six feet"
    );
}

/// The pad-check screen lights a key plate `OK` and writes `ACCENT_ON` on it
/// (`screens.rs`, `Lit::Full`), and fades it to `tint(OK, 70)` with `OK` text.
/// Neither pair is obvious from reading `theme.rs`.
#[test]
fn lit_key_plates_clear_the_floor() {
    let mut r = Report::default();
    r.check(
        TEXT_FLOOR,
        "ACCENT_ON on OK (Lit::Full)",
        role::ACCENT_ON,
        role::OK,
    );
    for (gn, g) in grounds() {
        r.check(
            TEXT_FLOOR,
            &format!("OK on tint(OK,70) over {gn} (Lit::Fading)"),
            role::OK,
            over(role::tint(role::OK, 70), g),
        );
        r.check(
            TEXT_FLOOR,
            &format!("TEXT on tint(COOL,70) over {gn} (selected slot row)"),
            role::TEXT,
            over(role::tint(role::COOL, 70), g),
        );
    }
    r.finish("lit key plates");
}

/// `DANGER_FILL` is drawn by nothing today, which is exactly why it needs a
/// test: the next person to wire up `Stop` will reach for it and should not
/// have to re-derive whether it is legible.
#[test]
fn the_reserved_stop_plate_is_legible() {
    let mut r = Report::default();
    r.check(
        TEXT_FLOOR,
        "TEXT on DANGER_FILL (reserved for Stop)",
        role::TEXT,
        role::DANGER_FILL,
    );
    r.finish("reserved plates");
}

#[test]
fn separators_stay_perceptible() {
    let mut r = Report::default();
    for (gn, g) in grounds() {
        r.check(
            SEPARATOR_FLOOR,
            &format!("BORDER_STRONG on {gn}"),
            role::BORDER_STRONG,
            g,
        );
    }
    r.check(
        SEPARATOR_FLOOR,
        "BORDER on RAISED",
        role::BORDER,
        role::RAISED,
    );
    r.finish("separators");
}

/// The two surfaces must agree about the accent. They diverge on *size* on
/// purpose — body 28 vs 14, hero 68 vs 38 — and that divergence is documented;
/// they must not diverge on what "you are here" looks like.
#[test]
fn the_accent_matches_the_web_surface() {
    let css = include_str!("../../../studio-ui/src/studio.css");
    let want = css
        .lines()
        .find_map(|l| l.trim().strip_prefix("--accent:"))
        .map(|v| v.trim().trim_end_matches(';').to_ascii_lowercase())
        .expect("studio.css must declare --accent");
    let got = format!(
        "#{:02x}{:02x}{:02x}",
        role::ACCENT.r(),
        role::ACCENT.g(),
        role::ACCENT.b()
    );
    assert_eq!(
        got, want,
        "cabinet ACCENT and the web --accent must be the same colour: one app, \
         one meaning for 'live/where you are'"
    );
}
