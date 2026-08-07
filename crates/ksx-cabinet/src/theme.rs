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

    // ── The Street Fighter palette, 10-foot translation ──────────────────
    //
    // Same decision as the web surface and for the same measured reason:
    // **purple is the GROUND, not the accent.** Violet-as-accent scores
    // 4.20:1 on the panel and fails the 4.5 floor; the lavenders that pass
    // are the ground's own hue, so they stop reading as "not the ground".
    // Teal stays, and near-complementary teal on violet actually measures
    // BETTER than it did on the old blue-steel (9.96:1 vs 9.75:1).
    //
    // What does NOT come across from the mark: its four player colours. P2
    // is red there, and red means Stop on this surface — a red slot 2 would
    // read as "slot 2 has failed". Slot identity is `COOL`, as it was.
    //
    // Every pair below is measured by `ksx-cabinet/tests/contrast.rs`,
    // including the composed ones (a state colour on its own `tint()`, text
    // on the focus plate), because those are what a person actually reads
    // and they are not visible in a list of constants.

    /// Page ground.
    pub const SURFACE: Color32 = Color32::from_rgb(0x12, 0x0c, 0x1c);
    /// A panel.
    pub const RAISED: Color32 = Color32::from_rgb(0x1c, 0x14, 0x28);
    /// Something nested inside a panel — a list row, a key chip.
    pub const INSET: Color32 = Color32::from_rgb(0x24, 0x1a, 0x33);

    /// Never pure white: it blooms on a TV panel, which is what this is
    /// (DESIGN-SYSTEM §3 "Contrast", stated there for the same reason).
    /// Cream rather than blue-white, because on a violet ground a cool white
    /// reads as a second hue rather than as "unstyled text".
    pub const TEXT: Color32 = Color32::from_rgb(0xf0, 0xeb, 0xe0);
    pub const TEXT_2: Color32 = Color32::from_rgb(0xbc, 0xb0, 0xc9);
    pub const TEXT_3: Color32 = Color32::from_rgb(0x8f, 0x83, 0xa3);

    pub const BORDER: Color32 = Color32::from_rgb(0x3b, 0x2c, 0x52);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x54, 0x40, 0x6f);

    /// **Accent — and on this surface it means ONE thing: where you are.**
    ///
    /// The web system lets accent mean live / primary / selected / bound at
    /// once, and on a dense page with a mouse that is fine, because the
    /// pointer disambiguates. With no pointer, a second meaning for the accent
    /// is a second thing that looks like the cursor. So: live is `OK`,
    /// selected-but-not-focused is a border, and the accent is the focus and
    /// nothing else.
    ///
    /// Now byte-identical to the web `--accent`. It used to be `#2fd4c1`, a
    /// shade off `#2fd8c5` for no recorded reason — two surfaces of one app
    /// disagreeing about the colour of "you are here" by an amount too small
    /// to be intentional and too large to be nothing.
    pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0xd8, 0xc5);
    /// Text drawn ON the accent plate. 9.20:1 — this pair is the cursor, so
    /// it is the one that must never soften.
    pub const ACCENT_ON: Color32 = Color32::from_rgb(0x04, 0x24, 0x1f);

    /// State. Dot + tint + full-strength text, never a large solid fill —
    /// except `DANGER_FILL`, which exists for exactly one control (Stop).
    pub const OK: Color32 = Color32::from_rgb(0x4e, 0xd1, 0x85);
    pub const WARN: Color32 = Color32::from_rgb(0xed, 0xbb, 0x46);
    /// **Deliberately lighter than the web's `--danger` (`#f4675f`)** and
    /// left where it was. The web value measures 4.44:1 against its own
    /// 15 % tint on `INSET` — under the floor — because this surface tints
    /// harder than the page does (`tint(colour, 38)`, ~15 %, over a plate
    /// that is already lifted). At 10 feet the lighter red is the right call
    /// anyway; the number just confirms it.
    pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
    /// Reserved for `Stop`, and currently drawn by nothing — kept because
    /// the role exists in the system. 7.44:1 against `TEXT`.
    pub const DANGER_FILL: Color32 = Color32::from_rgb(0x8e, 0x1f, 0x25);
    /// Identity — a device, a persona, a slot. NOT the accent, because "which
    /// player is this?" and "where am I?" are different questions. Violet, so
    /// identity belongs to the ground's family and cannot be mistaken for the
    /// cursor.
    pub const COOL: Color32 = Color32::from_rgb(0xa9, 0x8a, 0xe0);

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
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, role::BORDER);
    visuals.widgets.inactive.bg_fill = role::INSET;
    visuals.widgets.hovered.bg_fill = role::INSET;
    visuals.widgets.active.bg_fill = role::INSET;
    visuals.selection.bg_fill = role::ACCENT;
    visuals.selection.stroke = Stroke::new(1.0_f32, role::ACCENT_ON);
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
