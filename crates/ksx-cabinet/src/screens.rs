//! The five screens, and the chrome around them.
//!
//! Every screen is a **list of things that already exist**, and every confirm
//! is one `ksx-api` verb. There is no mapper here, no macro editor and no
//! preset file management, and there never will be: the cabinet OPERATES —
//! choose among things that exist — and Studio AUTHORS. The test each screen
//! had to pass is the same one: *joystick, two buttons, no text entry, ten
//! seconds, standing at the cab with a game about to start.*
//!
//! Nothing in this file decides anything a CLI verb does not already decide.
//! It places rows and colours them.

use std::time::Instant;

use egui::{Align, Color32, Layout, RichText, Sense, Stroke, Vec2};

use crate::app::{flash_alive, App, Ask, Lit, Slow, Tone};
use crate::nav::{Confirm, Screen};
use crate::theme::{ctl, fs, radius, role, sp, FOCUS_OFFSET, FOCUS_STROKE};

/// The top dock: who we are, where we are, and the state of the machine.
pub fn head(ui: &mut egui::Ui, app: &App, slow: &Slow) {
    header(ui, app, slow);
    ui.add_space(sp::S2);
    tabs(ui, app.focus.screen);
}

/// The bottom dock: the answer to the last press, and the legend.
pub fn foot(ui: &mut egui::Ui, app: &App, slow: &Slow, now: Instant) {
    if let Some(flash) = &slow.flash {
        if flash_alive(flash, now) {
            flash_line(ui, flash);
            ui.add_space(sp::S2);
        }
    }
    footer(ui, app, slow);
}

/// The screen itself, in whatever is left.
pub fn body(ui: &mut egui::Ui, app: &mut App, slow: &Slow, now: Instant) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match app.focus.screen {
            Screen::ButtonCheck => button_check(ui, app, slow, now),
            Screen::Status => status(ui, app, slow),
            Screen::Session => session(ui, app, slow),
            Screen::Profiles => profiles(ui, app, slow),
            Screen::Presets => presets(ui, app, slow),
        });
}

fn header(ui: &mut egui::Ui, app: &App, slow: &Slow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("ksx")
                .size(fs::LG)
                .color(role::ACCENT)
                .strong(),
        );
        ui.add_space(sp::S2);
        ui.label(
            RichText::new(app.focus.screen.label())
                .size(fs::LG)
                .color(role::TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (text, tone) = if !slow.session.reachable {
                ("NO DAEMON", Tone::Bad)
            } else if slow.session.running {
                ("RUNNING", Tone::Ok)
            } else {
                ("STOPPED", Tone::Warn)
            };
            pill(ui, text, tone);
            if slow.busy {
                ui.add_space(sp::S2);
                pill(ui, "WORKING", Tone::Warn);
            }
        });
    });
    ui.label(
        RichText::new(app.focus.screen.subtitle())
            .size(fs::SM)
            .color(role::TEXT_3),
    );
}

/// The screen strip. The current screen is a filled plate; the others are
/// borders. It is NOT the accent — the accent means the cursor, and only the
/// cursor (theme::role::ACCENT).
fn tabs(ui: &mut egui::Ui, current: Screen) {
    ui.horizontal(|ui| {
        for screen in Screen::ALL {
            let here = screen == current;
            let (fill, text) = if here {
                (role::INSET, role::TEXT)
            } else {
                (Color32::TRANSPARENT, role::TEXT_3)
            };
            let label = RichText::new(screen.label()).size(fs::MD).color(text);
            let galley = ui.painter().layout_no_wrap(
                screen.label().to_owned(),
                egui::FontId::proportional(fs::MD),
                text,
            );
            let size = Vec2::new(galley.size().x + sp::S4, ctl::ROW * 0.7);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter()
                .rect_filled(rect, crate::theme::plate(radius::ROW), fill);
            if here {
                ui.painter().rect_stroke(
                    rect,
                    crate::theme::plate(radius::ROW),
                    Stroke::new(2.0_f32, role::BORDER_STRONG),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label.text(),
                egui::FontId::proportional(fs::MD),
                text,
            );
        }
    });
}

/// The always-visible legend. On a surface with no mouse this is not help
/// text, it is the control panel — and it never scrolls away.
///
/// **ASCII only, on purpose.** egui's bundled fonts cover a specific set and
/// nothing warns you when they do not: a first pass used `▲ ▼ Ⓐ Ⓑ` and every
/// one of them drew as a tofu box on this machine, which on a cabinet is a
/// legend that says nothing at all. `< > ^ v A B` is guaranteed by every font
/// that exists, reads at six feet, and is what an arcade panel is labelled
/// with anyway.
fn footer(ui: &mut egui::Ui, app: &App, slow: &Slow) {
    let hint = if app.focus.confirming.is_some() {
        "<  >   choose            A   answer            B   no"
    } else if app.picking.is_some() {
        "^  v   preset            A   use it            B   back"
    } else {
        "<  >   screen            ^  v   move            A   choose            B   back"
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(hint).size(fs::SM).color(role::TEXT_2));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (pads, ever) = app.pad_nav_state();
            // **The one case that has to be loud.** With a session running the
            // panel is captured below win32k, so it produces no keystrokes at
            // all — the ONLY way it can move this cursor is as an XInput pad
            // (`crate::pad`). If nothing answers XInput (every slot on a
            // PlayStation persona, a HIDMaestro persona, or the pads not up
            // yet) then the panel cannot drive this window and there is no
            // keyboard either. "keyboard only" would be exactly backwards.
            let (text, colour) = if pads > 0 {
                (format!("panel: {pads} pad(s)"), role::TEXT_3)
            } else if slow.session.running {
                (
                    "panel: NO XInput pad — the panel cannot move this cursor while emulation \
                     runs (use a keyboard, or stop emulation)"
                        .to_owned(),
                    role::WARN,
                )
            } else if ever {
                ("panel: no pads right now".to_owned(), role::TEXT_3)
            } else {
                ("panel: keyboard only".to_owned(), role::TEXT_3)
            };
            ui.label(RichText::new(text).size(fs::XS).color(colour));
        });
    });
}

fn flash_line(ui: &mut egui::Ui, flash: &crate::app::Flash) {
    let colour = tone_colour(flash.tone);
    // `->`, not `→`. Every glyph this surface DRAWS is ASCII, for the reason
    // `footer` records: egui's bundled fonts cover a specific set, nothing
    // warns you when a codepoint is outside it, and a tofu box on a cabinet
    // screen is a legend that says nothing. U+2192 was never screenshotted
    // (a flash needs an action, and `--demo` refuses every one), so it was
    // never verified — and "probably in the font" is not a standard this
    // surface gets to use.
    let text = match &flash.remedy {
        Some(remedy) => format!("{}   ->  {remedy}", flash.text),
        None => flash.text.clone(),
    };
    let height = ctl::ROW;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(
        rect,
        crate::theme::plate(radius::ROW),
        role::tint(colour, 38),
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(6.0, rect.height())),
        crate::theme::plate(radius::CHIP),
        colour,
    );
    ui.painter().text(
        rect.left_center() + Vec2::new(sp::S3, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(fs::SM),
        colour,
    );
}

fn tone_colour(tone: Tone) -> Color32 {
    match tone {
        Tone::Ok => role::OK,
        Tone::Warn => role::WARN,
        Tone::Bad => role::DANGER,
    }
}

fn pill(ui: &mut egui::Ui, text: &str, tone: Tone) {
    let colour = tone_colour(tone);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(fs::MICRO),
        colour,
    );
    let size = Vec2::new(galley.size().x + sp::S4, ctl::ROW * 0.55);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(
        rect,
        crate::theme::plate(radius::ROW),
        role::tint(colour, 40),
    );
    ui.painter()
        .circle_filled(rect.left_center() + Vec2::new(sp::S2, 0.0), 6.0, colour);
    ui.painter().text(
        rect.left_center() + Vec2::new(sp::S2 + 14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(fs::MICRO),
        colour,
    );
}

/// One selectable row: the plate, the focus ring, the caret, the label.
///
/// **This is the cursor.** Three redundant signals, because on a cabinet panel
/// any one of them can be defeated — a washed-out screen kills the fill, a
/// photograph kills the colour, colour vision kills the hue. See
/// `theme`'s module docs.
fn row(ui: &mut egui::Ui, focused: bool, height: f32, draw: impl FnOnce(&mut egui::Ui, Color32)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let fill = if focused { role::ACCENT } else { role::INSET };
    let text = if focused { role::ACCENT_ON } else { role::TEXT };
    ui.painter()
        .rect_filled(rect, crate::theme::plate(radius::ROW), fill);
    if focused {
        ui.painter().rect_stroke(
            rect.expand(FOCUS_OFFSET),
            crate::theme::plate(radius::ROW),
            Stroke::new(FOCUS_STROKE, role::ACCENT),
            egui::StrokeKind::Outside,
        );
        // The third focus signal, and the one that survives a monochrome
        // panel, a photograph and a font with no arrows in it (see `footer`).
        ui.painter().text(
            rect.left_center() + Vec2::new(sp::S2, 0.0),
            egui::Align2::LEFT_CENTER,
            ">",
            egui::FontId::monospace(fs::BASE),
            text,
        );
    }
    let inner = rect.shrink2(Vec2::new(sp::S6, sp::S1));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    draw(&mut child, text);
}

/// One line of key-value text, for the facts that are read rather than
/// pressed.
fn fact(ui: &mut egui::Ui, name: &str, value: &str, colour: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).size(fs::SM).color(role::TEXT_3));
        ui.add_space(sp::S2);
        ui.label(RichText::new(value).size(fs::SM).color(colour));
    });
}

// ---------------------------------------------------------------------------
// a. Button check — the spine
// ---------------------------------------------------------------------------

/// Press a panel button, see it light, big.
///
/// Two columns, and the two columns are the point (docs/MAPPER-UX.md Build C,
/// and the TAS lesson behind it): **what the panel sent** and **what the pad
/// published** are different facts, and which of them moved tells you which
/// half is broken.
///
/// - neither moves → the key is not reaching ksx at all: wiring, or the board
///   is not the one this slot is bound to;
/// - left moves, right does not → the key arrives and is bound to nothing;
/// - right moves with no left → a macro or a chord, or another player's key.
fn button_check(ui: &mut egui::Ui, app: &mut App, slow: &Slow, now: Instant) {
    if let Some(why) = app.feed_unavailable() {
        app.focus.set_rows(0);
        stopped_notice(ui, &why, slow);
        return;
    }

    // The slot column is STABLE: it comes from the configuration, not from
    // whatever happened to be published in the last 16 ms. A row that appears
    // and vanishes as somebody presses things is unreadable, and "P3 is not
    // there at all" is itself a finding the operator needs to be able to see.
    let numbers: Vec<u8> = if slow.mapper.slots.is_empty() {
        app.frame.slots.iter().map(|slot| slot.slot).collect()
    } else {
        slow.mapper.slots.iter().map(|slot| slot.number).collect()
    };
    app.focus.set_rows(numbers.len());

    // `columns`, not two hand-sized children. A hand-sized child has to be
    // told a HEIGHT, and the honest height inside a scroll area is "as much as
    // the content needs" — give it the viewport's instead and a short window
    // clips both columns to nothing, silently. That is the failure this
    // surface can least afford: a button check that draws an empty panel is
    // indistinguishable from a panel that is not wired.
    ui.columns(2, |columns| {
        panel_column(&mut columns[0], app, now);
        pad_column(&mut columns[1], app, &numbers, now);
    });
}

/// The physical half: keys, newest first, with the board they came from.
fn panel_column(ui: &mut egui::Ui, app: &App, now: Instant) {
    eyebrow(ui, "THE PANEL SENT");
    ui.add_space(sp::S2);
    if app.keys.is_empty() {
        ui.label(
            RichText::new("press a button on the panel")
                .size(fs::MD)
                .color(role::TEXT_3),
        );
        return;
    }
    for (hit, at) in &app.keys {
        let fresh = now.saturating_duration_since(*at) < std::time::Duration::from_millis(450);
        let (fill, colour) = if fresh {
            (role::tint(role::COOL, 70), role::TEXT)
        } else {
            (role::INSET, role::TEXT_2)
        };
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), ctl::ROW * 0.8),
            Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, crate::theme::plate(radius::ROW), fill);
        ui.painter().text(
            rect.left_center() + Vec2::new(sp::S3, 0.0),
            egui::Align2::LEFT_CENTER,
            &hit.key,
            egui::FontId::monospace(fs::LIGHT),
            colour,
        );
        // WHICH BOARD. Two panels sending the same key are two facts, and on a
        // four-player cabinet that distinction is the whole diagnosis.
        let source = if hit.alias.is_empty() {
            short_device(&hit.device)
        } else {
            hit.alias.clone()
        };
        ui.painter().text(
            rect.right_center() - Vec2::new(sp::S3, 0.0),
            egui::Align2::RIGHT_CENTER,
            source,
            egui::FontId::proportional(fs::XS),
            role::COOL,
        );
    }
}

/// The virtual half: one row per slot, with whatever that pad is doing.
fn pad_column(ui: &mut egui::Ui, app: &App, numbers: &[u8], now: Instant) {
    eyebrow(ui, "THE PAD PUBLISHED");
    ui.add_space(sp::S2);
    if numbers.is_empty() {
        ui.label(
            RichText::new("no slots are configured")
                .size(fs::MD)
                .color(role::TEXT_3),
        );
        return;
    }
    // Lights INSIDE the row, not under it. Under it was clearer per slot and
    // wrong for the screen: four slots plus their chips ran past the bottom of
    // the panel, so P3 and P4 — the two a cabinet operator is most often
    // checking, because P1 obviously works — were below the fold with nothing
    // saying they existed. Every slot fits now, which is the requirement.
    for (index, number) in numbers.iter().enumerate() {
        let focused = app.focus.is_on(index);
        let lit = app.lit_controls(*number, now);
        row(ui, focused, ctl::ROW, |ui, text| {
            ui.label(
                RichText::new(format!("P{number}"))
                    .size(fs::LG)
                    .color(if focused { text } else { role::COOL })
                    .strong(),
            );
            ui.add_space(sp::S3);
            if lit.is_empty() {
                // An honest "nothing", not an empty row: a blank plate reads
                // as a rendering bug, a dash reads as a fact.
                ui.label(RichText::new("--").size(fs::BASE).color(if focused {
                    text
                } else {
                    role::TEXT_3
                }));
            }
            for (control, how) in &lit {
                light(ui, control, *how, focused);
            }
        });
    }
    if app.dropped > 0 {
        ui.add_space(sp::S2);
        fact(
            ui,
            "events dropped while this window was busy",
            &app.dropped.to_string(),
            role::WARN,
        );
    }
    // The desk keyboard, said out loud. The feed filters to the devices bound
    // to a slot in the running session, so a board that is not on this cabinet
    // lights nothing here — which is the whole point, and would be indis-
    // tinguishable from a dead panel if the count were not on screen.
    if app.off_panel > 0 {
        ui.add_space(sp::S2);
        fact(
            ui,
            "key(s) from a keyboard bound to NO slot (not shown above)",
            &app.off_panel.to_string(),
            role::TEXT_3,
        );
    }
}

/// One lit control. Full = happening; fading = just happened.
///
/// Monospace, and in the preset's own vocabulary (`A`, `dpad.up`, `lt`), so
/// that the word lighting up here is character-for-character the word a legend
/// row or a `ksx map --function` would use. A friendlier label would be a
/// second name for one thing.
///
/// `on_accent` inverts the plate. Green-on-teal was the first pass and it was
/// backwards in the worst possible place: the FOCUSED slot — the one somebody
/// is deliberately inspecting — had the least readable lights on the screen.
/// On the accent plate the chip goes dark with light text, which is the same
/// contrast in the other direction.
fn light(ui: &mut egui::Ui, control: &str, how: Lit, on_accent: bool) {
    let (fill, text) = match (how, on_accent) {
        (Lit::Full, false) => (role::OK, role::ACCENT_ON),
        (Lit::Fading, false) => (role::tint(role::OK, 70), role::OK),
        (Lit::Full, true) => (role::SURFACE, role::OK),
        (Lit::Fading, true) => (role::tint(role::SURFACE, 150), role::TEXT_2),
    };
    let galley =
        ui.painter()
            .layout_no_wrap(control.to_owned(), egui::FontId::monospace(fs::BASE), text);
    let size = Vec2::new(galley.size().x + sp::S3, ctl::ROW * 0.62);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, crate::theme::plate(radius::ROW), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        control,
        egui::FontId::monospace(fs::BASE),
        text,
    );
}

/// The button check with nothing running. A surface that cannot act must SAY
/// so — this is that, at the size the rest of the screen is.
fn stopped_notice(ui: &mut egui::Ui, why: &str, slow: &Slow) {
    ui.add_space(sp::S4);
    ui.label(
        RichText::new("Nothing to watch yet")
            .size(fs::XL)
            .color(role::TEXT),
    );
    ui.add_space(sp::S2);
    ui.label(RichText::new(why).size(fs::MD).color(role::TEXT_2));
    ui.add_space(sp::S3);
    if slow.session.reachable {
        // ASCII, like the legend and for the same reason: `◀ ▶ Ⓐ` all drew as
        // tofu boxes here (see `footer`). This line survived the first sweep
        // because `demo::DemoFeed` never reports `unavailable`, so no
        // screenshot could reach it — and it is the FIRST thing a real cabinet
        // shows when emulation is stopped, which is when a legend matters most.
        ui.label(
            RichText::new("Go to Start / Stop  ( < > )  and press A.")
                .size(fs::MD)
                .color(role::ACCENT),
        );
    } else {
        ui.label(
            RichText::new(&slow.session.line)
                .size(fs::MD)
                .color(role::DANGER),
        );
    }
}

fn eyebrow(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(fs::MICRO).color(role::TEXT_3));
}

/// `HID\VID_D209&PID_0430&MI_00\8&2A…` → `VID_D209&PID_0430`. A cabinet
/// screen has no room for an instance path, and the vendor/product pair is the
/// half that identifies the board.
fn short_device(device: &str) -> String {
    device
        .split('\\')
        .nth(1)
        .map(|middle| middle.split("&MI_").next().unwrap_or(middle).to_owned())
        .unwrap_or_else(|| device.to_owned())
}

// ---------------------------------------------------------------------------
// b. Am I working
// ---------------------------------------------------------------------------

fn status(ui: &mut egui::Ui, app: &mut App, slow: &Slow) {
    app.focus.set_rows(0);
    ui.label(
        RichText::new(&slow.session.line)
            .size(fs::HERO)
            .color(if slow.session.running {
                role::OK
            } else if slow.session.reachable {
                role::WARN
            } else {
                role::DANGER
            })
            .strong(),
    );
    ui.add_space(sp::S3);

    let pads = slow.snapshot.pads.len();
    ui.horizontal(|ui| {
        pill(
            ui,
            &format!("{pads} PAD(S) ON THE BUS"),
            if pads > 0 { Tone::Ok } else { Tone::Warn },
        );
        let (connected, _) = app.pad_nav_state();
        pill(
            ui,
            &format!("{connected} DRIVING THIS PANEL"),
            if connected > 0 { Tone::Ok } else { Tone::Warn },
        );
    });
    ui.add_space(sp::S2);

    // Tighter than the default row rhythm on purpose: these six are a REFERENCE
    // block, read one at a time by somebody already looking for one of them,
    // and at the page's normal spacing the last two fell off the bottom of the
    // panel — which on a status screen means they may as well not exist.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = sp::S1;
        fact(ui, "ViGEmBus", &slow.snapshot.vigem, role::TEXT);
        fact(ui, "Interception", &slow.snapshot.interception, role::TEXT);
        fact(ui, "Autostart", &slow.snapshot.autostart, role::TEXT_2);
        fact(ui, "Daemon", &slow.snapshot.daemon_detail, role::TEXT_2);
        // Where and when, on one line. Two lines of provenance is one line more
        // than a cabinet screen can spare, and neither half means much without
        // the other.
        fact(
            ui,
            "Config",
            &format!(
                "{}   (read {})",
                slow.snapshot.config_root, slow.snapshot.generated_at
            ),
            role::TEXT_3,
        );
    });

    ui.add_space(sp::S3);
    eyebrow(ui, "FROM YOUR PHONE");
    // NO QR. `serve` refuses anything but loopback, so a code could only ever
    // carry 127.0.0.1 — which is this machine, not the phone holding the
    // camera. A dead end that looks like a feature is worse than the sentence.
    ui.label(
        RichText::new(
            "Not available yet. ksx Studio serves 127.0.0.1 only — there is no LAN mode \
             and no pairing token, so a phone on this network cannot reach it.",
        )
        .size(fs::SM)
        .color(role::TEXT_2),
    );
}

// ---------------------------------------------------------------------------
// c. Start / stop, pick a game profile, pick a slot's preset
// ---------------------------------------------------------------------------

fn session(ui: &mut egui::Ui, app: &mut App, slow: &Slow) {
    app.focus.set_rows(2);
    let running = slow.session.running;
    let primary = if running {
        "Stop emulation"
    } else {
        "Start emulation"
    };
    let detail = if running {
        "the panel goes back to being a keyboard".to_owned()
    } else {
        match &slow.session.profile {
            Some(profile) => format!("profile: {profile}"),
            None => "the slots in config.toml".to_owned(),
        }
    };
    row(ui, app.focus.is_on(0), ctl::ROW_LG, |ui, text| {
        ui.label(RichText::new(primary).size(fs::XL).color(text).strong());
        ui.add_space(sp::S3);
        ui.label(RichText::new(detail).size(fs::SM).color(text));
    });
    ui.add_space(sp::S2);
    row(ui, app.focus.is_on(1), ctl::ROW, |ui, text| {
        ui.label(RichText::new("Reload config").size(fs::LG).color(text));
        ui.add_space(sp::S3);
        ui.label(
            RichText::new("stop, re-read the files, start again")
                .size(fs::SM)
                .color(text),
        );
    });

    if !slow.session.reachable {
        ui.add_space(sp::S3);
        ui.label(
            RichText::new(&slow.session.line)
                .size(fs::MD)
                .color(role::DANGER),
        );
    }
}

fn profiles(ui: &mut egui::Ui, app: &mut App, slow: &Slow) {
    let rows = &slow.snapshot.profiles;
    app.focus.set_rows(rows.len());
    if rows.is_empty() {
        ui.label(
            RichText::new("No game profiles in games.toml.")
                .size(fs::LG)
                .color(role::TEXT_2),
        );
        ui.add_space(sp::S2);
        ui.label(
            RichText::new(
                "This cabinet starts from the [[slot]] entries in config.toml. Profiles are \
                 authored in ksx Studio or by editing games.toml.",
            )
            .size(fs::SM)
            .color(role::TEXT_3),
        );
        return;
    }
    for (index, profile) in rows.iter().enumerate() {
        let current = slow.session.profile.as_deref() == Some(profile.title.as_str());
        row(ui, app.focus.is_on(index), ctl::ROW, |ui, text| {
            ui.label(
                RichText::new(&profile.title)
                    .size(fs::LG)
                    .color(text)
                    .strong(),
            );
            ui.add_space(sp::S3);
            ui.label(RichText::new(&profile.detail).size(fs::XS).color(text));
            if current {
                ui.add_space(sp::S3);
                ui.label(RichText::new("[ IN USE ]").size(fs::SM).color(text));
            }
        });
        ui.add_space(sp::S2);
    }
}

fn presets(ui: &mut egui::Ui, app: &mut App, slow: &Slow) {
    match app.picking {
        None => preset_slots(ui, app, slow),
        Some(slot) => preset_choices(ui, app, slow, slot),
    }
}

fn preset_slots(ui: &mut egui::Ui, app: &mut App, slow: &Slow) {
    let slots = &slow.mapper.slots;
    app.focus.set_rows(slots.len());
    if slots.is_empty() {
        ui.label(
            RichText::new(&slow.mapper.source)
                .size(fs::MD)
                .color(role::TEXT_2),
        );
        return;
    }
    eyebrow(ui, &slow.mapper.source.to_uppercase());
    if let Some(warning) = destination_mismatch(slow) {
        ui.add_space(sp::S1);
        ui.label(RichText::new(warning).size(fs::XS).color(role::WARN));
    }
    ui.add_space(sp::S2);
    for (index, slot) in slots.iter().enumerate() {
        row(ui, app.focus.is_on(index), ctl::ROW, |ui, text| {
            ui.label(
                RichText::new(format!("P{}", slot.number))
                    .size(fs::LG)
                    .color(text)
                    .strong(),
            );
            ui.add_space(sp::S3);
            ui.label(RichText::new(&slot.preset).size(fs::LG).color(text));
            ui.add_space(sp::S3);
            ui.label(
                RichText::new(format!("{} · {}", slot.persona_label, slot.keyboard))
                    .size(fs::XS)
                    .color(text),
            );
        });
        ui.add_space(sp::S2);
    }
}

fn preset_choices(ui: &mut egui::Ui, app: &mut App, slow: &Slow, slot: u8) {
    app.focus.set_rows(slow.presets.len());
    ui.label(
        RichText::new(format!("Which preset for P{slot}?"))
            .size(fs::XL)
            .color(role::TEXT),
    );
    ui.add_space(sp::S2);
    if let Some(why) = &slow.presets_error {
        ui.label(RichText::new(why).size(fs::MD).color(role::DANGER));
        return;
    }
    if slow.presets.is_empty() {
        ui.label(
            RichText::new("There are no presets on disk to choose from.")
                .size(fs::MD)
                .color(role::TEXT_2),
        );
        return;
    }
    let current = slow
        .mapper
        .slots
        .iter()
        .find(|s| s.number == slot)
        .map(|s| s.preset.clone())
        .unwrap_or_default();
    for (index, preset) in slow.presets.iter().enumerate() {
        let in_use = *preset == current;
        row(ui, app.focus.is_on(index), ctl::ROW, |ui, text| {
            ui.label(RichText::new(preset).size(fs::LG).color(text));
            if in_use {
                ui.add_space(sp::S3);
                ui.label(RichText::new("[ IN USE ]").size(fs::SM).color(text));
            }
        });
        ui.add_space(sp::S1);
    }
}

// ---------------------------------------------------------------------------
// Activation — every arm is one ksx-api verb, or one honest refusal
// ---------------------------------------------------------------------------

/// What Ⓐ means on each screen.
pub fn activate(app: &mut App, slow: &Slow, row: usize) {
    match app.focus.screen {
        // The button check watches; the one thing it can DO is forget what it
        // saw, which is what somebody re-wiring a panel wants between tests.
        Screen::ButtonCheck => {
            app.clear_log();
            app.say("cleared — press a panel button", Tone::Ok);
        }
        Screen::Status => app.say(
            "This screen only reports. Start / Stop is one screen to the right.",
            Tone::Warn,
        ),
        Screen::Session => match row {
            0 if slow.session.running => app.ask(Ask::Stop),
            0 => app.ask(Ask::Start(slow.session.profile.clone())),
            _ => app.ask(Ask::Reload),
        },
        Screen::Profiles => {
            let Some(profile) = slow.snapshot.profiles.get(row) else {
                return;
            };
            if slow.session.running {
                // Deliberately NOT a compound stop-then-start behind one
                // press. Switching profiles while a game is up is a bigger
                // thing than this surface should do quietly, and the daemon
                // would refuse the start anyway ("already running").
                app.say(
                    format!(
                        "\"{}\" needs emulation stopped first — Start / Stop is two screens left.",
                        profile.title
                    ),
                    Tone::Warn,
                );
                return;
            }
            app.ask(Ask::Start(Some(profile.title.clone())));
        }
        Screen::Presets => match app.picking {
            None => {
                let Some(slot) = slow.mapper.slots.get(row) else {
                    return;
                };
                app.picking = Some(slot.number);
                app.focus.set_rows(slow.presets.len());
            }
            Some(slot) => {
                let Some(preset) = slow.presets.get(row) else {
                    return;
                };
                let profile = assign_destination(slow);
                // THE ONE MODAL. A slot assignment is the only thing on this
                // surface that interrupts a running game, and it says so in
                // the words the verb itself uses (ksx_api::SlotOutcome::
                // restarted): the pads replug.
                let consequence = if slow.session.running {
                    "The session RESTARTS: all four pads unplug and plug back in. \
                     Anything mid-game will see its controllers vanish."
                        .to_owned()
                } else {
                    "Nothing is running, so nothing restarts — the next start uses it.".to_owned()
                };
                app.confirm(
                    format!("Use \"{preset}\" for P{slot}?"),
                    consequence,
                    Ask::Assign {
                        slot,
                        preset: preset.clone(),
                        profile,
                    },
                );
            }
        },
    }
}

/// Which file a slot assignment lands in: **the one the list on screen was
/// read from**, and nothing else.
///
/// This used to read `SessionView::profile` — "the profile the daemon is
/// pointed at" — on the reasoning that it matches what a start would use. It
/// is the wrong answer, and wrong in the most damaging direction available to
/// this verb. The two are genuinely different facts: `StatusSource::mapper`
/// reads `config.toml`'s `[[slot]]` entries when there are any and falls back
/// to the first games.toml profile otherwise, while the session's profile is
/// whatever `--game` said. A daemon started as `ksx daemon --game "Steam"` on
/// a config that ALSO has `[[slot]]` entries therefore listed config.toml's
/// slots and wrote games.toml's — repointing a row nobody was looking at, and
/// CREATING one (default persona, no keyboard) when that profile had no such
/// slot.
///
/// A picker must write to the file it read from. Where the session disagrees,
/// [`preset_slots`] says so on screen rather than quietly choosing one.
fn assign_destination(slow: &Slow) -> Option<String> {
    slow.mapper.profile.clone()
}

/// The session is running slots from a different file than the one this screen
/// is showing. Not a refusal — the write is still correct for what is on
/// screen — but the user has to be told that changing it will not move the
/// session they are looking at.
fn destination_mismatch(slow: &Slow) -> Option<String> {
    if !slow.session.reachable || slow.mapper.slots.is_empty() {
        return None;
    }
    if slow.session.profile == slow.mapper.profile {
        return None;
    }
    let showing = match &slow.mapper.profile {
        Some(profile) => format!("profile \"{profile}\""),
        None => "config.toml".to_owned(),
    };
    let running = match &slow.session.profile {
        Some(profile) => format!("profile \"{profile}\""),
        None => "config.toml".to_owned(),
    };
    Some(format!(
        "These slots are {showing}, but the daemon starts from {running}. A change here \
         edits {showing} — it will not move the session unless you point the daemon at it."
    ))
}

/// The one modal. Full-screen dim, one question, two answers, and **No is
/// where the cursor starts** (`nav::Focus::ask`).
pub fn modal(ctx: &egui::Context, confirm: &Confirm) {
    egui::Area::new("ksx-cabinet-modal".into())
        .fixed_pos(egui::Pos2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            ui.painter()
                .rect_filled(screen, 0, Color32::from_black_alpha(220));
            let card = egui::Rect::from_center_size(
                screen.center(),
                Vec2::new(
                    (screen.width() * 0.8).min(1100.0),
                    (screen.height() * 0.6).min(560.0),
                ),
            );
            ui.painter()
                .rect_filled(card, crate::theme::plate(radius::CARD), role::RAISED);
            ui.painter().rect_stroke(
                card,
                crate::theme::plate(radius::CARD),
                Stroke::new(2.0_f32, role::BORDER_STRONG),
                egui::StrokeKind::Inside,
            );

            let inner = card.shrink(sp::S6);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::LEFT)),
            );
            child.label(
                RichText::new(&confirm.question)
                    .size(fs::XL)
                    .color(role::TEXT)
                    .strong(),
            );
            child.add_space(sp::S3);
            child.label(
                RichText::new(&confirm.consequence)
                    .size(fs::MD)
                    .color(role::WARN),
            );
            child.add_space(sp::S6);
            child.horizontal(|ui| {
                answer(ui, "No", !confirm.yes);
                ui.add_space(sp::S4);
                answer(ui, "Yes — do it", confirm.yes);
            });
        });
}

fn answer(ui: &mut egui::Ui, text: &str, focused: bool) {
    let size = Vec2::new(340.0, ctl::ROW_LG);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let fill = if focused { role::ACCENT } else { role::INSET };
    let colour = if focused { role::ACCENT_ON } else { role::TEXT };
    ui.painter()
        .rect_filled(rect, crate::theme::plate(radius::ROW), fill);
    if focused {
        ui.painter().rect_stroke(
            rect.expand(FOCUS_OFFSET),
            crate::theme::plate(radius::ROW),
            Stroke::new(FOCUS_STROKE, role::ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(fs::LG),
        colour,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_api::{MapperSnapshot, SessionView};

    fn slow(mapper_profile: Option<&str>, session_profile: Option<&str>) -> Slow {
        Slow {
            session: SessionView {
                reachable: true,
                running: true,
                line: "running".into(),
                profile: session_profile.map(str::to_owned),
            },
            mapper: MapperSnapshot {
                generated_at: "t".into(),
                source: "s".into(),
                profile: mapper_profile.map(str::to_owned),
                config_root: "r".into(),
                slots: vec![ksx_api::MapperSlot {
                    number: 1,
                    persona: "xbox360".into(),
                    persona_label: "Xbox 360".into(),
                    preset: "IPAC P1".into(),
                    keyboard: "(any)".into(),
                    bindings: Default::default(),
                    backup: None,
                    turbo: Default::default(),
                    macros_off: false,
                }],
            },
            ..Slow::default()
        }
    }

    /// **The write goes where the READ came from.** Taking the destination
    /// from the session instead meant a daemon started `--game "Steam"` on a
    /// config that also has `[[slot]]` entries listed config.toml's slots and
    /// wrote games.toml's — repointing a row nobody was looking at, and
    /// creating one where the profile had no such slot.
    #[test]
    fn a_slot_assignment_lands_in_the_file_the_list_was_read_from() {
        assert_eq!(assign_destination(&slow(None, Some("Steam"))), None);
        assert_eq!(
            assign_destination(&slow(Some("MAME"), Some("Steam"))).as_deref(),
            Some("MAME")
        );
    }

    /// ...and when the two disagree the screen SAYS so, because a change that
    /// will not move the running session is not what "Slots" looks like it
    /// promises.
    #[test]
    fn a_list_from_a_different_file_than_the_session_is_flagged_on_screen() {
        assert!(destination_mismatch(&slow(Some("Steam"), Some("Steam"))).is_none());
        assert!(destination_mismatch(&slow(None, None)).is_none());
        let warning = destination_mismatch(&slow(None, Some("Steam"))).expect("a mismatch");
        assert!(warning.contains("config.toml"), "{warning}");
        assert!(warning.contains("Steam"), "{warning}");
        // Nothing to warn about when there is nothing on screen to change.
        let empty = Slow {
            mapper: MapperSnapshot {
                slots: Vec::new(),
                ..slow(None, Some("Steam")).mapper
            },
            ..slow(None, Some("Steam"))
        };
        assert!(destination_mismatch(&empty).is_none());
    }

    /// Every string this file DRAWS is ASCII, or one of two codepoints that
    /// have been SEEN rendering in a screenshot.
    ///
    /// egui's bundled fonts cover a specific set, nothing warns you when a
    /// codepoint is outside it, and the first pass shipped four tofu boxes
    /// into the one legend a mouseless surface has. A screenshot cannot catch
    /// the ones on paths `--demo` cannot reach — `stopped_notice` needs
    /// `LiveFeed::unavailable`, a flash needs an action the demo refuses, and
    /// the WORKING pill needs a verb in flight — which is exactly where the
    /// survivors were found. So it is asserted instead of looked at.
    #[test]
    fn nothing_this_surface_draws_is_an_unverified_codepoint() {
        // Verified by eye, in the screenshots named:
        //   U+2014 EM DASH      — cab-2-status.png, "127.0.0.1 only — there is…"
        //   U+00B7 MIDDLE DOT   — cab-5-slots.png,  "Xbox 360 · IPAC 2"
        // Nothing goes in this list that has not been looked at.
        const SEEN: [char; 2] = ['\u{2014}', '\u{00b7}'];
        let source = include_str!("screens.rs");
        for (number, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with("/*") || code.starts_with('*') {
                continue;
            }
            // This test's own prose (the table above) is not drawn.
            if code.starts_with("const SEEN") {
                continue;
            }
            for character in line.chars() {
                assert!(
                    character.is_ascii() || SEEN.contains(&character),
                    "line {} draws U+{:04X} ({character}), which no screenshot has verified: \
                     {line}",
                    number + 1,
                    character as u32
                );
            }
        }
    }
}
