//! **The contrast contract, enforced.**
//!
//! `docs/DESIGN-SYSTEM.md` §3 says text is ≥ 4.5:1 against the surface it sits
//! on, in both themes. That sentence has been true only by inspection: every
//! ratio in that document was measured once, by hand, at the moment it was
//! written, and nothing has re-measured them since. Tokens drift — a number in
//! a doc does not defend itself.
//!
//! So this file re-derives every ratio from the CSS that actually ships, on
//! every `cargo test`. It parses `studio-ui/src/studio.css` rather than
//! restating its values, because a test that hardcodes the palette is a second
//! copy of the palette and would drift in exactly the same way.
//!
//! # What it checks, and why those pairs
//!
//! Not the cartesian product of colours and grounds — that would fail on
//! combinations the app never draws and force the palette darker than the
//! design needs. The ground sets below are the ones that **actually compose**
//! in `studio.css`:
//!
//! - **Text tiers** sit on all four panel grounds, and on `--panel-3` when a
//!   control is hovered. `--panel-3` carries only `--text`/`--text-2`: every
//!   `:hover` rule that sets it also sets one of those two
//!   (`.btn:hover`, `.tab:hover`, `.navlink:active`), so `--text-3` is not
//!   checked against it.
//! - **Coloured roles** sit on the four panel grounds — including
//!   `--bg-2`, which is a real card ground (`.legendcard`, `.macrocard`,
//!   `.hint`, `.grid .card`, `.strow.sthead`), not just a page wash. They are
//!   never drawn on `--panel-3`.
//! - **A role on its own tint** is the state triad from §3 ("dot + tint +
//!   full-strength text"), and it is the case hand-measurement misses: a pill
//!   is accent text on `--accent-dim`, not accent text on the panel. Three of
//!   the four real defects this file was written to catch were of this shape.
//!
//! # Floors
//!
//! | Pair | Floor | Source |
//! |---|---|---|
//! | anything that renders as text | 4.5 | WCAG 2.2 AA, and DESIGN-SYSTEM §3 |
//! | the focus ring against its ground | 3.0 | WCAG 2.2 AA non-text (1.4.11) |
//! | a separator against its ground | 1.12 | calibrated, see below |
//!
//! The 1.12 separator floor is **not** a WCAG number. Hairline borders in this
//! system are decorative — §5's "dark mode separates by surface lightness + a
//! hairline border" — and the perceivable boundary that 1.4.11 actually cares
//! about is the focus ring, which is held to 3.0 above. 1.12 is the faintest
//! separator in the theme that shipped and was reviewed (dark
//! `--line-soft`/`--panel`), so it encodes "no worse than what we already
//! accepted" and catches a token going flat, without pretending to be an
//! accessibility threshold.

use std::collections::BTreeMap;

const CSS: &str = include_str!("../../../studio-ui/src/studio.css");
const BUILD_MJS: &str = include_str!("../../../studio-ui/build.mjs");
const RENDER_RS: &str = include_str!("../src/render.rs");
const RENDER_MAP_RS: &str = include_str!("../src/render_map.rs");

const TEXT_FLOOR: f64 = 4.5;
const NON_TEXT_FLOOR: f64 = 3.0;
/// See the module docs: calibrated to the outgoing theme, not to WCAG.
const SEPARATOR_FLOOR: f64 = 1.12;

// ── colour ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Rgba {
    fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f64,
            g: g as f64,
            b: b as f64,
            a: 1.0,
        }
    }

    /// Composite `self` over `bg`. The tint tokens are the whole reason this
    /// exists: `--accent-dim` is `rgba(accent, 0.13)`, and what a reader sees
    /// is that *over a ground*, which is neither of the two colours named.
    fn over(self, bg: Rgba) -> Rgba {
        Rgba {
            r: self.r * self.a + bg.r * (1.0 - self.a),
            g: self.g * self.a + bg.g * (1.0 - self.a),
            b: self.b * self.a + bg.b * (1.0 - self.a),
            a: 1.0,
        }
    }

    fn luminance(self) -> f64 {
        fn chan(c: f64) -> f64 {
            let s = c / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * chan(self.r) + 0.7152 * chan(self.g) + 0.0722 * chan(self.b)
    }
}

/// WCAG 2.x relative-contrast ratio. Both inputs must already be opaque.
fn ratio(fg: Rgba, bg: Rgba) -> f64 {
    let (a, b) = (fg.luminance(), bg.luminance());
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

fn parse_colour(raw: &str) -> Option<Rgba> {
    let raw = raw.trim();
    if let Some(h) = raw.strip_prefix('#') {
        let h = h.trim();
        return match h.len() {
            6 => Some(Rgba::opaque(
                u8::from_str_radix(&h[0..2], 16).ok()?,
                u8::from_str_radix(&h[2..4], 16).ok()?,
                u8::from_str_radix(&h[4..6], 16).ok()?,
            )),
            3 => {
                let d = |i: usize| -> Option<u8> {
                    let c = u8::from_str_radix(&h[i..=i], 16).ok()?;
                    Some(c * 17)
                };
                Some(Rgba::opaque(d(0)?, d(1)?, d(2)?))
            }
            _ => None,
        };
    }
    if let Some(rest) = raw.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f64> = rest
            .split(',')
            .map(|p| p.trim().parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        if parts.len() == 4 && !parts.iter().any(|v| v.is_nan()) {
            return Some(Rgba {
                r: parts[0],
                g: parts[1],
                b: parts[2],
                a: parts[3],
            });
        }
    }
    None
}

// ── parsing studio.css ───────────────────────────────────────────────────

/// The light theme lives in `@media (prefers-color-scheme: light)`. Everything
/// before it is the dark theme plus the theme-agnostic primitives; the media
/// block is an override layer on top.
fn split_themes(css: &str) -> (String, String) {
    let marker = "@media (prefers-color-scheme: light)";
    let start = css
        .find(marker)
        .expect("studio.css must contain the light-theme media query");
    let after = &css[start..];
    let open = after.find('{').expect("media query must open a block");
    let mut depth = 0usize;
    let mut end = None;
    for (i, ch) in after[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("light media query must close");
    (css[..start].to_owned(), after[open..=end].to_owned())
}

/// Every `--name: <colour>` declaration in `block`. Non-colour custom
/// properties (`--macrow: 1.9rem`) simply do not parse and are skipped.
fn tokens(block: &str) -> BTreeMap<String, Rgba> {
    let mut out = BTreeMap::new();
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        // Several tokens carry a trailing `/* … */` note on the same line
        // (`--accent-on: #04241f; /* text ON an accent fill */`), so the
        // comment has to come off before the semicolon does.
        let value = value.split("/*").next().unwrap_or(value);
        let value = value.trim().trim_end_matches(';').trim();
        if let Some(c) = parse_colour(value) {
            out.insert(name.trim().to_owned(), c);
        }
    }
    out
}

struct Theme {
    name: &'static str,
    tok: BTreeMap<String, Rgba>,
}

impl Theme {
    fn get(&self, name: &str) -> Rgba {
        *self
            .tok
            .get(name)
            .unwrap_or_else(|| panic!("{}: token --{name} is missing from studio.css", self.name))
    }

    /// The four grounds a panel-level colour can legitimately sit on.
    fn panel_grounds(&self) -> Vec<(&'static str, Rgba)> {
        vec![
            ("--bg (surface)", self.get("bg")),
            ("--bg-2 (sunken)", self.get("bg-2")),
            ("--panel", self.get("panel")),
            ("--panel-2 (inset)", self.get("panel-2")),
        ]
    }
}

fn themes() -> Vec<Theme> {
    let (dark_block, light_block) = split_themes(CSS);
    let dark = tokens(&dark_block);
    let mut light = dark.clone();
    light.extend(tokens(&light_block));
    vec![
        Theme {
            name: "dark",
            tok: dark,
        },
        Theme {
            name: "light",
            tok: light,
        },
    ]
}

// ── the report ───────────────────────────────────────────────────────────

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, floor: f64, theme: &str, label: &str, fg: Rgba, bg: Rgba) {
        let v = ratio(fg, bg);
        self.lines
            .push(format!("  {v:6.2}  (>={floor:.2})  {theme:<5} {label}"));
        if v < floor {
            self.failures
                .push(format!("{theme}: {label} = {v:.2}:1, floor {floor:.2}:1"));
        }
    }

    fn finish(self, what: &str) {
        // Always print the measurements: the point of this test is the table,
        // not the boolean. `cargo test -- --nocapture` is the design review.
        println!("── {what} ──");
        for l in &self.lines {
            println!("{l}");
        }
        assert!(
            self.failures.is_empty(),
            "\n{} contrast failure(s) in {what}:\n  {}\n\n\
             Fix the TOKEN, not the component rule that revealed it \
             (DESIGN-SYSTEM §13.1). If a pair is a deliberate exemption, it \
             belongs in this file with its reason, not in a comment \
             somewhere else.\n",
            self.failures.len(),
            self.failures.join("\n  ")
        );
    }
}

// ── the checks ───────────────────────────────────────────────────────────

/// The three text tiers, on every ground that renders text.
#[test]
fn text_tiers_clear_the_floor_on_every_ground() {
    let mut r = Report::default();
    for t in themes() {
        for (gn, g) in t.panel_grounds() {
            for tier in ["text", "text-2", "text-3"] {
                r.check(
                    TEXT_FLOOR,
                    t.name,
                    &format!("--{tier} on {gn}"),
                    t.get(tier),
                    g,
                );
            }
        }
        // `--panel-3` is the control fill under the cursor, and it gets ALL
        // THREE tiers — no carve-out.
        //
        // It used to get only --text and --text-2, justified by "every rule
        // that sets --panel-3 also sets --text or --text-2". That was an
        // unreachability claim about the markup, made in a comment, and this
        // stylesheet already contained its counter-example: `.tab .tabnum` is
        // a DESCENDANT with its own `--text-tertiary`, so `.tab:hover`'s
        // `color:` never reaches it. It is not rendered today, which is the
        // only reason light `--text-3` on `--panel-3` (4.41:1) was not
        // shipping — a fact about markup that no longer exists, guarding a
        // pair in the stylesheet that still does. The token was corrected
        // instead; this loop is what keeps it corrected.
        let hover = t.get("panel-3");
        for tier in ["text", "text-2", "text-3"] {
            r.check(
                TEXT_FLOOR,
                t.name,
                &format!("--{tier} on --panel-3 (hover)"),
                t.get(tier),
                hover,
            );
        }
    }
    r.finish("text tiers");
}

/// Accent, identity and the three state colours, drawn as text.
#[test]
fn coloured_roles_clear_the_floor_as_text() {
    let mut r = Report::default();
    for t in themes() {
        for (gn, g) in t.panel_grounds() {
            for role in ["accent", "accent-strong", "cool", "ok", "warn", "danger"] {
                r.check(
                    TEXT_FLOOR,
                    t.name,
                    &format!("--{role} on {gn}"),
                    t.get(role),
                    g,
                );
            }
        }
    }
    r.finish("coloured roles as text");
}

/// The state triad: full-strength text on its own tint, over each ground.
/// This is the pair that hand-measurement misses — and the one that caught
/// the light accent (3.33:1 on `--accent-dim-2` over `--bg-2`).
#[test]
fn role_text_clears_the_floor_on_its_own_tint() {
    let mut r = Report::default();
    for t in themes() {
        for (gn, g) in t.panel_grounds() {
            for (role, tint) in [
                ("accent", "accent-dim"),
                ("accent", "accent-dim-2"),
                ("ok", "ok-dim"),
                ("warn", "warn-dim"),
                ("danger", "danger-dim"),
            ] {
                let composed = t.get(tint).over(g);
                r.check(
                    TEXT_FLOOR,
                    t.name,
                    &format!("--{role} on --{tint} over {gn}"),
                    t.get(role),
                    composed,
                );
            }
        }
    }
    r.finish("role text on its own tint");
}

/// Text drawn ON a solid role plate: the primary button and Stop.
#[test]
fn text_on_solid_role_plates_clears_the_floor() {
    let mut r = Report::default();
    for t in themes() {
        r.check(
            TEXT_FLOOR,
            t.name,
            "--accent-on over --accent-fill (.btn-primary)",
            t.get("accent-on"),
            t.get("accent-fill"),
        );
        r.check(
            TEXT_FLOOR,
            t.name,
            "--danger-on over --danger-fill (Stop)",
            t.get("danger-on"),
            t.get("danger-fill"),
        );
        r.check(
            TEXT_FLOOR,
            t.name,
            "--accent-on over --accent-strong (:hover)",
            t.get("accent-on"),
            t.get("accent-strong"),
        );
    }
    r.finish("text on solid plates");
}

/// The focus ring is the one non-text element with a real accessibility
/// floor (WCAG 1.4.11): it must be perceivable on every surface it can land
/// on, including a hovered control.
#[test]
fn focus_ring_clears_the_non_text_floor() {
    let mut r = Report::default();
    for t in themes() {
        let mut grounds = t.panel_grounds();
        grounds.push(("--panel-3 (hover)", t.get("panel-3")));
        for (gn, g) in grounds {
            r.check(
                NON_TEXT_FLOOR,
                t.name,
                &format!("--focus on {gn}"),
                t.get("focus"),
                g,
            );
        }
    }
    r.finish("focus ring");
}

/// Separators must stay perceptible. Calibrated, not a WCAG floor — see the
/// module docs.
#[test]
fn separators_stay_perceptible() {
    let mut r = Report::default();
    for t in themes() {
        for (line, grounds) in [
            ("line", vec!["panel", "bg", "panel-2"]),
            ("line-soft", vec!["panel"]),
            ("line-strong", vec!["panel"]),
        ] {
            for g in grounds {
                r.check(
                    SEPARATOR_FLOOR,
                    t.name,
                    &format!("--{line} on --{g}"),
                    t.get(line),
                    t.get(g),
                );
            }
        }
    }
    r.finish("separators");
}

/// The controller art's printed markings.
///
/// `--hw-xbox-b` and `--hw-xbox-x` are a **recorded exemption**, not an
/// oversight: white on those two reds/blues is what the physical pad prints,
/// the art is a picture of a device rather than a label, and every hit zone
/// carries an `aria-label` naming the control and its bound keys
/// (`render_map.rs`). Pinning them here means changing them is a decision
/// somebody has to make on purpose.
#[test]
fn hardware_markings_are_either_legible_or_a_recorded_exemption() {
    let (dark_block, _) = split_themes(CSS);
    let t = Theme {
        name: "hw",
        tok: tokens(&dark_block),
    };
    let mut r = Report::default();

    // The pairs that must clear the text floor.
    r.check(
        TEXT_FLOOR,
        "hw",
        "xbox A letter on its disc",
        t.get("hw-xbox-a-on"),
        t.get("hw-xbox-a"),
    );
    r.check(
        TEXT_FLOOR,
        "hw",
        "xbox Y letter on its disc",
        t.get("hw-xbox-y-on"),
        t.get("hw-xbox-y"),
    );
    for glyph in ["circle", "cross", "triangle", "square"] {
        r.check(
            TEXT_FLOOR,
            "hw",
            &format!("PS {glyph} on --hw-ps-disc"),
            t.get(&format!("hw-ps-{glyph}")),
            t.get("hw-ps-disc"),
        );
    }
    r.finish("hardware markings");

    // The exemption, pinned. If someone repaints these, this fires and they
    // have to come back to the comment above and decide again.
    for (name, expected) in [("hw-xbox-b", 4.00_f64), ("hw-xbox-x", 3.94_f64)] {
        let got = ratio(t.get("hw-xbox-on"), t.get(name));
        assert!(
            (got - expected).abs() < 0.02,
            "--{name} is a recorded contrast exemption measured at {expected:.2}:1, \
             but now measures {got:.2}:1. That is fine if it went UP, and a \
             decision either way — update the exemption in \
             ksx-studio/tests/contrast.rs and the note in studio.css §1."
        );
    }
}

/// Placeholder text, which is **not** exempt from the text floor.
///
/// `input::placeholder` takes `--text-tertiary` and sits on the field's own
/// `--surface-inset`, inside a card or the page. It carries meaning ("what
/// goes here"), it is not an inactive control, and nothing else in this file
/// measured it.
#[test]
fn placeholder_text_clears_the_text_floor() {
    let mut r = Report::default();
    for t in themes() {
        // The field is `--panel-2`; the card/page grounds are where a field
        // with a transparent-ish inset can still be read against.
        for ground in ["panel-2", "panel", "bg"] {
            r.check(
                TEXT_FLOOR,
                t.name,
                &format!("--text-3 placeholder on --{ground}"),
                t.get("text-3"),
                t.get(ground),
            );
        }
    }
    r.finish("placeholder text");
}

/// Disabled controls — a **recorded exemption**, pinned.
///
/// WCAG 2.2 SC 1.4.3 exempts "text ... that is part of an inactive user
/// interface component", so these are conformant at any ratio; raising them
/// to 4.5 would defeat the affordance, because "dimmer than the live
/// controls" is precisely what communicates the state. They are pinned rather
/// than skipped because §13.7 says an exemption ships with its number.
///
/// There are TWO live compositions, not one. This test originally claimed
/// there was only the first, and that claim was wrong in the way that
/// matters — it named the harmless one and missed the whole-screen one.
///
/// 1. The status page's daemon-down block (`StatusIsland.ts`) renders
///    `<div class="controls off">` holding a `<select disabled>` and a
///    `<button class="btn" disabled>Start</button>` inside a `.card`. Two
///    rules set opacity on that button — `.controls.off .btn` (0.40) and
///    `.btn:disabled` (0.42) — and the three-class selector wins, so
///    **0.40 is the number that renders**, not 0.42.
///
/// 2. **The whole mapper, whenever a session is running.** `render_map.rs`
///    stamps `z-dead` on every one of the 25 pad zones and `l-dead` on
///    every one of the 25 legend rows, and `.zone.z-dead, .lrow.l-dead`
///    sets `opacity: 0.4`. So the controller art AND the bindings legend —
///    the entire content of the screen — render at 0.40 in the read-only
///    state. Measured in a real browser (Playwright, 1600px, dark): a bound
///    key `.lkc.lk1` composites to **2.55:1**, a key chip on the art
///    `.ztag` to **2.39:1**, a control label `.lid.id-txt` to **1.79:1**.
///
/// Case 2 is pinned here so the number is on record, but it deserves a
/// decision rather than a pin. The 1.4.3 exemption is written for an
/// *inactive component* — a Start button you cannot press. The bindings
/// legend is not a component the user operates, it is REFERENCE TEXT they
/// read, and "which key is B?" is a question worth asking precisely while a
/// session is running. Two loud banners already say the screen is read-only
/// (`.alarm` + `.warnbox`, both at full strength), so the dimming is the
/// third telling — and it is the one that costs the screen its content.
/// Dimming only the interactive affordance (`.zone`) and leaving the legend
/// at full strength would keep the affordance and the information both.
///
/// CSS `opacity` composites the element's whole buffer over its parent, so
/// the text AND the field fade together; that is why this cannot be measured
/// as a plain token pair.
///
/// What must NOT be dimmed is the reason: the `<p class="warn">` in the same
/// block is deliberately outside both selectors, and is checked at full
/// strength by `coloured_roles_clear_the_floor_as_text`.
#[test]
fn disabled_controls_are_a_pinned_exemption() {
    /// `.controls.off .btn` / `.controls.off select`.
    const DISABLED_OPACITY: f64 = 0.40;

    fn faded(c: Rgba, alpha: f64, parent: Rgba) -> Rgba {
        Rgba {
            r: c.r * alpha + parent.r * (1.0 - alpha),
            g: c.g * alpha + parent.g * (1.0 - alpha),
            b: c.b * alpha + parent.b * (1.0 - alpha),
            a: 1.0,
        }
    }

    // Measured 2026-08-06. Dark reads noticeably better than light because
    // fading toward a bright parent collapses the pair faster.
    for (theme_name, expected) in [("dark", 3.45_f64), ("light", 2.36_f64)] {
        let t = themes()
            .into_iter()
            .find(|t| t.name == theme_name)
            .expect("theme present");
        let card = t.get("panel");
        let label = faded(t.get("text"), DISABLED_OPACITY, card);
        let field = faded(t.get("panel-2"), DISABLED_OPACITY, card);
        let got = ratio(label, field);
        assert!(
            (got - expected).abs() < 0.02,
            "{theme_name}: the disabled control label measures {got:.2}:1, pinned at \
             {expected:.2}:1. This pair is a WCAG 1.4.3 exemption (inactive \
             component), NOT a free pass — if it moved, decide again and update \
             both this pin and DESIGN-SYSTEM §3.5."
        );
    }

    // Case 2: the read-only mapper. `.zone.z-dead, .lrow.l-dead { opacity: .4 }`
    // over the legend's own ground. `--bg-2` is the ground the legend card
    // actually draws on (`.legendcard`), which is why it is the one measured.
    const DEAD_OPACITY: f64 = 0.40;
    for (theme_name, role, expected) in [
        ("dark", "accent", 2.55_f64), // a bound key in the legend
        ("dark", "text-3", 1.81_f64), // a control's own label
        ("light", "accent", 1.85_f64),
        ("light", "text-3", 1.70_f64),
    ] {
        let t = themes()
            .into_iter()
            .find(|t| t.name == theme_name)
            .expect("theme present");
        let ground = t.get("bg-2");
        let got = ratio(faded(t.get(role), DEAD_OPACITY, ground), ground);
        assert!(
            (got - expected).abs() < 0.05,
            "{theme_name}: --{role} in the READ-ONLY MAPPER measures {got:.2}:1 against \
             its own ground, pinned at {expected:.2}:1. This is the whole mapper — every \
             pad zone and every legend row carries z-dead/l-dead while a session runs. \
             It is pinned, not blessed: see this test's docs before changing the opacity \
             or the token, and update DESIGN-SYSTEM §3.5 with whatever is decided."
        );
    }
}

/// The anti-flash `<style>` is a hand copy of `--bg`/`--text`, so it drifts
/// silently: a wrong anti-flash colour looks exactly like the flash it exists
/// to prevent. It HAD drifted before this test existed (`#0b0e14` against a
/// stylesheet that had moved to `#0a0d13`).
#[test]
fn anti_flash_css_matches_the_tokens_it_mirrors() {
    let themes = themes();
    let dark = &themes[0];
    let light = &themes[1];

    let expect_dark = format!(
        "body{{background:{};color:{};margin:0}}",
        css_hex(dark.get("bg")),
        css_hex(dark.get("text"))
    );
    let expect_light = format!(
        "body{{background:{};color:{}}}",
        css_hex(light.get("bg")),
        css_hex(light.get("text"))
    );

    for (file, src) in [("render.rs", RENDER_RS), ("render_map.rs", RENDER_MAP_RS)] {
        let compact: String = src.chars().filter(|c| !c.is_whitespace()).collect();
        let want_dark: String = expect_dark.chars().filter(|c| !c.is_whitespace()).collect();
        let want_light: String = expect_light
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            compact.contains(&want_dark),
            "{file}: PERSONALITY_CSS must open with the dark tokens from \
             studio.css — expected to find `{expect_dark}`"
        );
        assert!(
            compact.contains(&want_light),
            "{file}: PERSONALITY_CSS's light override must match studio.css — \
             expected to find `{expect_light}`"
        );
    }
}

/// The controller art's palette is hand-mirrored into `build.mjs` because an
/// `<img>`-embedded SVG is its own document and inherits no custom properties.
/// Same drift risk as the anti-flash CSS, so same treatment.
#[test]
fn pad_art_palette_stays_separable() {
    let sheet = BUILD_MJS
        .split("const PAD_SHEET")
        .nth(1)
        .expect("build.mjs must define PAD_SHEET");
    let sheet = &sheet[..sheet
        .find("</style>")
        .expect("PAD_SHEET must close its <style>")];

    let grab = |class: &str, prop: &str, nth: usize| -> Rgba {
        let hits: Vec<&str> = sheet
            .match_indices(class)
            .map(|(i, _)| &sheet[i..])
            .collect();
        let block = hits.get(nth).unwrap_or_else(|| {
            panic!(
                "PAD_SHEET must declare {class} at least {} time(s)",
                nth + 1
            )
        });
        let after = &block[block
            .find(prop)
            .unwrap_or_else(|| panic!("PAD_SHEET's {class} (occurrence {nth}) must set {prop}"))..];
        let hex_start = after.find('#').expect("declaration must use a hex colour");
        parse_colour(&after[hex_start..hex_start + 7]).expect("valid hex colour")
    };

    let mut r = Report::default();
    // Occurrence 0 = dark, occurrence 1 = the prefers-color-scheme:light
    // override inside the same sheet.
    for (nth, theme) in [(0usize, "dark"), (1usize, "light")] {
        let body = grab(".pad-body{", "fill:", nth);
        let stroke = grab(".pad-body{", "stroke:", nth);
        let detail = grab(".pad-detail{", "fill:", nth);
        let inset = grab(".pad-inset{", "fill:", nth);
        // The art is a picture, not text: the floor is the non-text one for
        // the shapes that carry meaning (buttons against the body) and the
        // separator floor for the big slab that is only a region.
        r.check(
            NON_TEXT_FLOOR,
            theme,
            "pad detail on pad body",
            detail,
            body,
        );
        r.check(
            NON_TEXT_FLOOR,
            theme,
            "pad outline on pad body",
            stroke,
            body,
        );
        r.check(
            SEPARATOR_FLOOR,
            theme,
            "pad inset slab on pad body",
            inset,
            body,
        );
    }
    r.finish("controller art");

    // ── the mirrors, pinned ──────────────────────────────────────────────
    //
    // Separation alone does NOT catch drift: `PAD_SHEET` is a hand copy of
    // specific studio.css tokens, and a copy that wanders stays perfectly
    // separable while quietly disagreeing with the page around it. That is
    // how the light outline came to be `#6d6180` after `--text-3` had moved
    // to `#685c7a` — the art kept the old tertiary, and nothing noticed.
    //
    // Only the values that really ARE token mirrors are pinned. The dark
    // body fill, dark detail and both insets are bespoke art colours chosen
    // against the silhouette, not copies of anything, so asserting them
    // would be inventing a contract that was never there.
    let themes = themes();
    let dark = &themes[0];
    let light = &themes[1];
    for (nth, theme, class, prop, token, expect) in [
        (
            0usize,
            "dark",
            ".pad-body{",
            "stroke:",
            "text-3",
            dark.get("text-3"),
        ),
        (
            1usize,
            "light",
            ".pad-body{",
            "stroke:",
            "text-3",
            light.get("text-3"),
        ),
        (
            1usize,
            "light",
            ".pad-body{",
            "fill:",
            "panel-2",
            light.get("panel-2"),
        ),
        (
            1usize,
            "light",
            ".pad-detail{",
            "fill:",
            "text-2",
            light.get("text-2"),
        ),
    ] {
        let got = grab(class, prop, nth);
        assert_eq!(
            css_hex(got),
            css_hex(expect),
            "build.mjs PAD_SHEET: {theme} {class}{prop} is a hand mirror of \
             studio.css `--{token}` and has drifted — it reads {} but the token \
             is {}. Update PAD_SHEET (an <img> SVG is its own document and \
             cannot use var(), so the copy is unavoidable; letting it rot is \
             not).",
            css_hex(got),
            css_hex(expect)
        );
    }
}

fn css_hex(c: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        c.r.round() as u8,
        c.g.round() as u8,
        c.b.round() as u8
    )
}
