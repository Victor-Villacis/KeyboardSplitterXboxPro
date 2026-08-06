//! **The 10-foot translation of `docs/DESIGN-SYSTEM.md`.**
//!
//! Not a port. That document's numbers are correct *for a 14 px product UI on
//! a desk monitor*, and every one of them is wrong here — a cabinet screen is
//! a TV, viewed standing, from four to eight feet, by somebody who is not
//! going to lean in. What travels is the **reasoning**, and the reasoning is
//! four rules:
//!
//! | DESIGN-SYSTEM says | what it means at six feet |
//! |---|---|
//! | one type scale, body is 14 | one type scale, **body is 26**, and the state line is 68 |
//! | one accent, meaning live/primary/selected | one accent, meaning **exactly one thing: where you are** |
//! | colour is spent, not decorated; ok/warn/danger are state only | unchanged, and more so — there is no room for decoration |
//! | one focus ring, declared globally, never removed | **the single most important pixel on the screen** (see below) |
//!
//! # The type scale, and why it starts where it does
//!
//! The governing number is angular size, not pixels. A 1080p cabinet panel at
//! 6 feet gives roughly 25 px per 0.1° of arc; comfortable sustained reading
//! wants ~0.3° of cap height, and a glanceable label wants more. That puts
//! body text at 24–28 px and the one line you read while walking past at 60+.
//! Everything below is that, rounded to a scale rather than picked per widget
//! — which is the actual rule DESIGN-SYSTEM §1 is enforcing.
//!
//! # Focus
//!
//! On the web the focus ring is an accessibility obligation. Here it is the
//! **entire interaction model**: there is no mouse, so the ring is the cursor.
//! It gets three redundant signals at once, because one of them will be
//! defeated by some cabinet's washed-out panel or somebody's colour vision:
//!
//! 1. a filled plate in the accent colour — the largest area change on screen;
//! 2. a 5 px border two pixels outside it, so it reads on a light and a dark
//!    plate alike (DESIGN-SYSTEM §7's offset, scaled);
//! 3. a `>` caret in the leading gutter, which survives a monochrome panel and
//!    a photograph of one. ASCII, like every other glyph this surface draws —
//!    see `screens::footer` for why (`▸` would be one more unverified
//!    codepoint, and the first pass shipped four of those as tofu boxes).
//!
//! Anything less has to be *hunted for* from across a room, and hunting for
//! the cursor is the failure mode this surface exists to avoid.

use egui::{CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};

/// The type scale. Nine steps, same shape as DESIGN-SYSTEM §1, re-derived for
/// distance; nothing draws a size that is not one of these.
pub mod fs {
    /// Eyebrows, uppercase labels, the key/pad column headers.
    pub const MICRO: f32 = 18.0;
    /// Metadata, device ids, footnotes.
    pub const XS: f32 = 21.0;
    /// Dense rows — a slot line, a key chip.
    pub const SM: f32 = 24.0;
    /// **Body.** Menu items, list rows, help.
    pub const MD: f32 = 28.0;
    /// Emphasised body; the label of a primary action.
    pub const BASE: f32 = 32.0;
    /// Card and screen titles.
    pub const LG: f32 = 40.0;
    /// Section headline.
    pub const XL: f32 = 52.0;
    /// The one line read from across the room: "RUNNING — 4 pads".
    pub const HERO: f32 = 68.0;
    /// A single control's live label in the button check — the thing a person
    /// standing at the panel is staring at while they press it.
    pub const LIGHT: f32 = 46.0;
}

/// Spacing. 8 px atomic unit (twice the web's 4), same "nothing between steps"
/// rule as DESIGN-SYSTEM §2.
pub mod sp {
    pub const S1: f32 = 8.0;
    pub const S2: f32 = 16.0;
    pub const S3: f32 = 24.0;
    pub const S4: f32 = 32.0;
    pub const S6: f32 = 48.0;
    pub const S8: f32 = 64.0;
}

/// Control height. One ladder, and the default is a **72 px row** — a cabinet
/// is operated standing, sometimes with a hand still on the panel, and every
/// target here is also a touch target on the machines that have a touch panel.
pub mod ctl {
    pub const ROW: f32 = 72.0;
    pub const ROW_LG: f32 = 96.0;
}

/// The colour roles of DESIGN-SYSTEM §3, one dark set only.
///
/// One theme, not two, and that is a deliberate difference from the web
/// surface: a cabinet screen lives in a dim room next to a CRT bezel, and a
/// light theme on it is a lamp pointed at the player. The web page keeps both
/// because a desk at noon is a real place; this does not, because a cabinet at
/// noon is not.
pub mod role {
    use egui::Color32;

    /// Page ground.
    pub const SURFACE: Color32 = Color32::from_rgb(0x0d, 0x11, 0x17);
    /// A panel.
    pub const RAISED: Color32 = Color32::from_rgb(0x15, 0x1b, 0x24);
    /// Something nested inside a panel — a list row, a key chip.
    pub const INSET: Color32 = Color32::from_rgb(0x1d, 0x25, 0x30);

    /// Never pure white: it blooms on a TV panel, which is what this is
    /// (DESIGN-SYSTEM §3 "Contrast", stated there for the same reason).
    pub const TEXT: Color32 = Color32::from_rgb(0xe3, 0xe9, 0xf4);
    pub const TEXT_2: Color32 = Color32::from_rgb(0xa8, 0xb4, 0xc8);
    pub const TEXT_3: Color32 = Color32::from_rgb(0x7d, 0x8a, 0xa3);

    pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x31, 0x3f);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x47, 0x59);

    /// **Accent — and on this surface it means ONE thing: where you are.**
    ///
    /// The web system lets accent mean live / primary / selected / bound at
    /// once, and on a dense page with a mouse that is fine, because the
    /// pointer disambiguates. With no pointer, a second meaning for the accent
    /// is a second thing that looks like the cursor. So: live is `OK`,
    /// selected-but-not-focused is a border, and the accent is the focus and
    /// nothing else.
    pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0xd4, 0xc1);
    /// Text drawn ON the accent plate.
    pub const ACCENT_ON: Color32 = Color32::from_rgb(0x04, 0x14, 0x13);

    /// State. Dot + tint + full-strength text, never a large solid fill —
    /// except `DANGER_FILL`, which exists for exactly one control (Stop).
    pub const OK: Color32 = Color32::from_rgb(0x3f, 0xd6, 0x7a);
    pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xb1, 0x3c);
    pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
    pub const DANGER_FILL: Color32 = Color32::from_rgb(0x8e, 0x1f, 0x25);
    /// Identity — a device, a persona, a slot. NOT the accent, because "which
    /// player is this?" and "where am I?" are different questions.
    pub const COOL: Color32 = Color32::from_rgb(0x6f, 0x9a, 0xd8);

    /// A tint of `colour` at `alpha`, for the state triad's middle term.
    pub fn tint(colour: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
    }
}

/// Radius. A control's radius is never larger than its container's
/// (DESIGN-SYSTEM §4), scaled with everything else.
pub mod radius {
    pub const CHIP: u8 = 6;
    pub const ROW: u8 = 10;
    pub const CARD: u8 = 16;
}

/// The focus plate's border width. Deliberately thick: this is the cursor.
pub const FOCUS_STROKE: f32 = 5.0;
/// ...and the gap between the plate and its border, so the ring reads against
/// both the plate and the page (DESIGN-SYSTEM §7's `outline-offset`, scaled).
pub const FOCUS_OFFSET: f32 = 3.0;

/// Install the whole system on a context. Called once at startup.
pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(fs::XS, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(fs::MD, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(fs::BASE, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(fs::LG, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(fs::SM, FontFamily::Monospace),
        ),
    ]
    .into();

    let mut visuals = Visuals::dark();
    visuals.panel_fill = role::SURFACE;
    visuals.window_fill = role::RAISED;
    visuals.extreme_bg_color = role::SURFACE;
    visuals.override_text_color = Some(role::TEXT);
    visuals.widgets.noninteractive.bg_fill = role::RAISED;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, role::BORDER);
    visuals.widgets.inactive.bg_fill = role::INSET;
    visuals.widgets.hovered.bg_fill = role::INSET;
    visuals.widgets.active.bg_fill = role::INSET;
    visuals.selection.bg_fill = role::ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, role::ACCENT_ON);
    // egui's own focus ring is off: this surface draws its own, and two rings
    // disagreeing about where the cursor is would be worse than either.
    visuals.widgets.hovered.expansion = 0.0;
    visuals.widgets.active.expansion = 0.0;
    style.visuals = visuals;

    style.spacing.item_spacing = egui::vec2(sp::S2, sp::S2);
    style.spacing.button_padding = egui::vec2(sp::S3, sp::S2);
    style.spacing.window_margin = Margin::same(sp::S4 as i8);
    // **Deliberately NOT a global minimum row height.** The obvious thing is to
    // put [`ctl::ROW`] here so every control is a standing-up target — and it
    // costs a cabinet its screen, because egui applies `interact_size` to every
    // `horizontal`, including the ones that are pure text. The first pass did
    // exactly that and six status lines became 430 px, pushing four of them off
    // the panel. The ladder belongs to the COMPONENTS that are pressed
    // (`screens::row` allocates `ctl::ROW` explicitly); a line you read is not
    // a target you hit.
    style.spacing.interact_size = egui::vec2(0.0, 0.0);
    // A cabinet has no scroll wheel and no drag; the bars exist so a list that
    // is longer than the screen still SAYS it is longer than the screen.
    style.spacing.scroll.bar_width = 12.0;

    ctx.set_style(style);
}

/// The rounded plate every row, chip and card is drawn on. One helper so a
/// radius is never typed at a call site (DESIGN-SYSTEM §4's rule, enforced by
/// there being nowhere else to put a number).
pub fn plate(radius: u8) -> CornerRadius {
    CornerRadius::same(radius)
}
