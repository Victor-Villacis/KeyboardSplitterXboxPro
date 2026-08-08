//! The mapper page's render seam (`/map`): embedded FMIR + per-request
//! [`MapPayload`] → HTML. Same architecture as `render.rs` (SSR slots for
//! first paint, the identical payload as island props for hydration — dogfood
//! ledger #5), same compiler constraints, its own slot layout.
//!
//! # The zone model
//!
//! The controller art (Gamepad-Asset-Pack, MIT — see `studio-ui/art/README.md`)
//! is an `<img>` filling the bottom `ART_SHARE` of a fixed-aspect "stage";
//! the top band holds the LB/RB/LT/RT chips (the icon art does not draw
//! shoulders), stacked trigger-over-bumper and visually anchored to the body
//! silhouette below. Every mappable control is a HIT ZONE: an absolutely
//! positioned
//! `<button data-fn=…>` from the [`ZONE_XBOX`]/[`ZONE_DS4`] tables.
//!
//! v7 — **every zone wears its own IDENTITY** (Victor: "I can see G is mapped
//! to A but I can't see the A xbox button"). The vendored art is a line
//! drawing with no letters on it, so the zone renders the control's name
//! itself: a persona-aware glyph in the canonical colours (A green, B red,
//! X blue, Y amber; ✕ blue, ○ red, △ green, □ pink), LB/RB/LT/RT and
//! view/menu/guide as text chips, arrows for the dpad and the stick wedges.
//!
//! v16 — direction glyphs are UNIFIED ON ARROWS (`↑ ↓ ← →`, not `▲ ▼ ◀ ▶`)
//! everywhere, art included. The macro grid gives every direction group its
//! own eight-position ring (`↑ ↖ ← ↙ ↓ ↘ → ↗`), so ALL FOUR diagonals are
//! columns you can point at — a 360 walks every one of them — and `◤◥◣◢` are
//! corner *blocks*, not directions: a diagonal that did not look like the same
//! family as its two parents would defeat the whole lens (see `fold`, below).
//! The bound key rides UNDER the identity as the small mono `ztag` — identity
//! always, key tag whenever the stage is wide enough for it (a container query
//! drops the tag on a phone; the legend still carries every key). An unbound
//! control therefore still reads as a controller, which is the whole point.
//!
//! The readable truth is the bindings LEGEND below the stage: one row per
//! function (same identity glyph + group prefix + key tag), carrying the same
//! `data-fn` so a row click is exactly the zone click, and hover
//! cross-highlights via the client's shared hot signal. One `createList`
//! renders all 25 zones, another the 25 legend rows; geometry is data, not
//! markup.
//!
//! # Shared keys are information, never a conflict
//!
//! One key driving several controls is native to the engine (`preset.rs`: "one
//! key → many functions"; docs/INPUT-TRANSFORMS.md §1a) and is exactly what
//! v7's multi-select flow writes. Both readers say so instead of complaining:
//! a control whose key is also bound elsewhere in the same preset gets
//! `z-shared`/`l-shared` (a cool-toned key tag), and its legend row carries a
//! compact "also A · B" badge naming the co-bound controls.
//!
//! Zone coordinates are STAGE percentages, authored from the art's real
//! geometry (`studio-ui/art/extents.mjs` — the PadForge lesson: derive layout
//! from art with a script, never trace by eye) plus hand placement for the
//! shoulders and the small center buttons — which `build.mjs` now also draws
//! into the recolored art at the same coordinates. The tables
//! are mirrored in `studio-ui/src/MapIsland.ts` (client re-derivation per
//! poll — the established applyStatus pattern); `zone_tables_cover_every_
//! mappable_function` pins this side.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{art_for, body_prefix, daemon_command, with_icon_links, EmbeddedPage};
use crate::snapshot::{MacroStepView, MacroView, MapPayload, MapperSlot};

/// List slot names (binding-derived, compiler 0.2.0). The zones list appears
/// TWICE — once inside each stage show (xbox / ds4) — so the second
/// occurrence gets the `#2` suffix, exactly like the status page's
/// profileRows pair; both receive the same array.
const LIST_SLOT_TABS: &str = "list:slotTabs:array";
/// v14: the SAME slot array, rendered a second time as the preset surface's
/// "which slot binds which file" table. Compiler 0.2.0 suffixes repeat
/// bindings by document order, exactly as the status page's two profile-row
/// lists are named — so the rail is `slotTabs` and the table `slotTabs#2`.
const LIST_SLOT_TABS_2: &str = "list:slotTabs#2:array";
const LIST_SLOT_ZONES: &str = "list:zones:array";
const LIST_SLOT_ZONES_2: &str = "list:zones#2:array";
/// The legend reads its own signal (`() => legendRows()`), so it gets a
/// binding-derived name of its own — no third `zones` occurrence.
const LIST_SLOT_LEGEND: &str = "list:legendRows:array";
/// v8's toast stack. Always injected EMPTY: a toast reports an action, and
/// SSR has not taken one. The empty array still emits the list's
/// `<!--f:lN-->` markers, which is exactly what the client's adoption path
/// needs in order to insert toasts into it later — and it costs no new
/// `createShow` (ledger #4/#14), because the stack is a list inside a plain
/// container rather than a conditional panel.
const LIST_SLOT_TOASTS: &str = "list:toasts:array";
/// v11's macro editor. Four lists, all binding-derived like the rest: the tab
/// strip, the grid's column headers, the row bar (step number + duration +
/// flag + the five step verbs) and the FLAT `steps × controls` cell matrix.
const LIST_SLOT_MACRO_TABS: &str = "list:macroTabs:array";
/// v16: the group band above the glyph row. Document order puts it first
/// inside `.macscroll`, so it lands between the row bar and the columns.
const LIST_SLOT_MACRO_GROUPS: &str = "list:macroGroups:array";
const LIST_SLOT_MACRO_COLS: &str = "list:macroCols:array";
const LIST_SLOT_MACRO_ROWS: &str = "list:macroRows:array";
const LIST_SLOT_MACRO_CELLS: &str = "list:macroCells:array";

#[cfg(test)]
const ISLAND_COMPONENT: &str = "MapIsland";

/// How many `createShow` pairs the mapper has. Name-addressable since
/// compiler 0.3.1 (`show:<condition getter>`), so the `MAP_SHOW_ORDER`
/// positional array is gone along with the rule it enforced — "append a new
/// show, never insert one, or every boolean after it shifts" (ledger
/// #4/#14, adopted 2026-08-06). [`show_values`] yields `(slot name, value)`
/// pairs; the layout test pins the exact name set.
const MAP_SHOW_COUNT: usize = 19;

/// Bare-named slots the mapper renders and the seam deliberately never fills.
///
/// `modalTurboLine` is the learn modal's auto-fire sentence. It describes the
/// control the USER just picked in a dialog that is never SSR-open
/// (`show:modalOpen` is unconditionally false here), so the server has nothing
/// to say about it and its authored `""` is the honest paint. Every other
/// signal `MapIsland.ts` binds to the DOM is server-injected — see
/// [`crate::render::assert_island_slot_contract`] for why that list has to be
/// explicit rather than implied.
#[cfg(test)]
const MAP_CLIENT_ONLY_SLOTS: [&str; 1] = ["modalTurboLine"];

/// The anonymous (`attr:`/`text:`) slots this page compiles to — every one of
/// them a string CONCATENATION, and every one of them a live test of ledger
/// #20's fix.
///
/// Compiler 0.3.1 folds `"a" + "b"` instead of dropping it, but it folds it
/// into a slot's DEFAULT rather than into static markup: four concatenated
/// `title` attributes (the two 360 motion buttons, the reverse 360, and the
/// macro Enable switch) and one concatenated child (`.macstephint`). The seam
/// can never inject these — they render their default or they render nothing —
/// so the contract check pins the set AND asserts each default is non-empty.
/// A concatenation with a non-literal operand would land here with an empty
/// default, which is the silent failure #20(a) shipped.
#[cfg(test)]
const MAP_ANONYMOUS_SLOTS: [&str; 5] = [
    "attr:title",
    "attr:title#2",
    "attr:title#3",
    "attr:title#4",
    "text:0",
];

// The art `<img>` occupies the bottom 86% of the stage (`.padart` in
// studio.css); the top band holds the shoulder chips. Zone Y values below
// are authored as `14 + artY·0.86`.

/// One hit zone: canonical function, on-art identity label, identity palette,
/// stage-percent box, css variant.
pub(crate) struct Zone {
    pub fn_name: &'static str,
    /// What this control is CALLED on this persona — the identity drawn on the
    /// art ("A", "✕", "LB", "▲", "menu").
    pub label: &'static str,
    /// Identity palette class suffix (`id-<idk>` in studio.css): the Xbox face
    /// colours `xa`/`xb`/`xx`/`xy`, the Sony glyphs `pc`/`po`/`pt`/`psq`,
    /// `dir` (dpad + stick arrows), `hub` (L3/R3), `txt` (view/menu/guide,
    /// share/options/PS), `sh` (shoulders).
    pub idk: &'static str,
    /// Center, stage percent.
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// CSS variant class: round | chip | trigger | bumper.
    pub kind: &'static str,
}

/// `rect` is the stage-percent box as `[cx, cy, w, h]` — one argument so the
/// tables below read as columns, and so geometry stays a unit.
const fn zone(
    fn_name: &'static str,
    label: &'static str,
    idk: &'static str,
    rect: [f32; 4],
    kind: &'static str,
) -> Zone {
    Zone {
        fn_name,
        label,
        idk,
        cx: rect[0],
        cy: rect[1],
        w: rect[2],
        h: rect[3],
        kind,
    }
}

/// Xbox-series-style pad (art: `pad-xbox.svg`, viewBox 112.46×76.66; anchors
/// from extents.mjs: face Y(75.2,19.9) B(82.0,29.8) A(75.3,39.9) X(68.7,29.9),
/// Lstick(24.0,29.9), Rstick(62.5,51.6), dpad(36.4,53.4) — art Y mapped to
/// stage as 14 + y·0.86).
///
/// Rects are pairwise DISJOINT (pinned by `zone_tables_cover_every_mappable_
/// function`): face buttons sized to the drawn circles, and the four
/// stick-direction wedges RING the stick with the L3/R3 click zone as the
/// 8×10 center hub — adjacent, never covering it.
pub(crate) const ZONE_XBOX: [Zone; 25] = [
    // Shoulders (not drawn in the icon art): slim chips stacked trigger-over-
    // bumper like the real pad, anchored just above the body's top plateau
    // (stage x ≈ 32..68) — .z-bumper drops a connector line onto the body.
    zone("lt", "LT", "sh", [31.0, 4.6, 10.0, 5.2], "trigger"),
    zone("lb", "LB", "sh", [34.0, 10.9, 11.0, 5.2], "bumper"),
    zone("rb", "RB", "sh", [66.0, 10.9, 11.0, 5.2], "bumper"),
    zone("rt", "RT", "sh", [69.0, 4.6, 10.0, 5.2], "trigger"),
    // Face cluster (diamond — boxes trimmed to the drawn Ø7.3×9.1 circles so
    // the diagonal neighbours stay disjoint). Canonical Xbox colours.
    zone("Y", "Y", "xy", [75.2, 31.1, 7.2, 8.4], "round"),
    zone("B", "B", "xb", [82.0, 39.6, 7.2, 8.4], "round"),
    zone("A", "A", "xa", [75.3, 48.3, 7.2, 8.4], "round"),
    zone("X", "X", "xx", [68.7, 39.7, 7.2, 8.4], "round"),
    // Center cluster: guide up top, view/menu inboard below it.
    zone("guide", "guide", "txt", [50.0, 27.0, 9.0, 11.0], "round"),
    zone("back", "view", "txt", [44.0, 39.0, 6.5, 8.0], "chip"),
    zone("start", "menu", "txt", [56.0, 39.0, 6.5, 8.0], "chip"),
    // Left stick: L3 hub + four ring wedges hugging it.
    zone("lthumb", "L3", "hub", [24.0, 39.7, 8.0, 10.0], "round"),
    zone("ly.max", "↑", "dir", [24.0, 31.7, 7.0, 6.0], "chip"),
    zone("ly.min", "↓", "dir", [24.0, 47.7, 7.0, 6.0], "chip"),
    zone("lx.min", "←", "dir", [17.25, 39.7, 5.5, 7.0], "chip"),
    zone("lx.max", "→", "dir", [30.75, 39.7, 5.5, 7.0], "chip"),
    // Dpad cross.
    zone("dpad.up", "↑", "dir", [36.4, 50.6, 7.0, 9.0], "chip"),
    zone("dpad.down", "↓", "dir", [36.4, 69.2, 7.0, 9.0], "chip"),
    zone("dpad.left", "←", "dir", [29.2, 59.9, 7.0, 9.0], "chip"),
    zone("dpad.right", "→", "dir", [43.6, 59.9, 7.0, 9.0], "chip"),
    // Right stick: R3 hub + ring wedges.
    zone("rthumb", "R3", "hub", [62.5, 58.4, 8.0, 10.0], "round"),
    zone("ry.max", "↑", "dir", [62.5, 50.4, 7.0, 6.0], "chip"),
    zone("ry.min", "↓", "dir", [62.5, 66.4, 7.0, 6.0], "chip"),
    zone("rx.min", "←", "dir", [55.75, 58.4, 5.5, 7.0], "chip"),
    zone("rx.max", "→", "dir", [69.25, 58.4, 5.5, 7.0], "chip"),
];

/// DualShock 4 pad (art: `pad-ds4.svg`, viewBox 112.69×72.53; anchors:
/// △(81.2,17.7) ○(88.4,28.8) ✕(81.3,39.7) □(74.0,28.7), sticks (33.8,49.8)
/// and (66.1,49.8), dpad arrows around (18.5,29.3), touchpad x 32.9..67.0 —
/// Sony labels, XInput functions). Same disjoint-rect rules as [`ZONE_XBOX`]:
/// stick wedges ring the L3/R3 hub, dpad arrow boxes sit on the drawn arrows
/// pushed slightly outward so the diagonal pairs never intersect.
pub(crate) const ZONE_DS4: [Zone; 25] = [
    // Shoulders: same trigger-over-bumper stack as ZONE_XBOX, anchored on the
    // DS4 body's raised humps (stage x ≈ 19 / 81, where L1/R1 really sit).
    zone("lt", "L2", "sh", [17.0, 4.6, 9.5, 5.2], "trigger"),
    zone("lb", "L1", "sh", [19.5, 10.9, 10.5, 5.2], "bumper"),
    zone("rb", "R1", "sh", [80.5, 10.9, 10.5, 5.2], "bumper"),
    zone("rt", "R2", "sh", [83.0, 4.6, 9.5, 5.2], "trigger"),
    // Face cluster (✕○△□ mapped onto A/B/Y/X), trimmed to the Ø6.9×9.2
    // drawn circles. Sony glyph colours.
    zone("Y", "△", "pt", [81.2, 29.2, 7.0, 9.0], "round"),
    zone("B", "○", "po", [88.4, 38.8, 7.0, 9.0], "round"),
    zone("A", "✕", "pc", [81.3, 48.1, 7.0, 9.0], "round"),
    zone("X", "□", "psq", [74.0, 38.7, 7.0, 9.0], "round"),
    // Share / PS / Options.
    zone("back", "share", "txt", [30.0, 25.5, 7.0, 9.0], "chip"),
    zone("start", "options", "txt", [70.0, 25.5, 7.0, 9.0], "chip"),
    zone("guide", "PS", "txt", [50.0, 63.0, 8.0, 10.0], "round"),
    // Left stick: L3 hub + ring wedges.
    zone("lthumb", "L3", "hub", [33.8, 56.8, 8.0, 10.0], "round"),
    zone("ly.max", "↑", "dir", [33.8, 48.8, 7.0, 6.0], "chip"),
    zone("ly.min", "↓", "dir", [33.8, 64.8, 7.0, 6.0], "chip"),
    zone("lx.min", "←", "dir", [27.05, 56.8, 5.5, 7.0], "chip"),
    zone("lx.max", "→", "dir", [40.55, 56.8, 5.5, 7.0], "chip"),
    // Dpad arrows.
    zone("dpad.up", "↑", "dir", [18.5, 31.5, 5.4, 7.2], "chip"),
    zone("dpad.down", "↓", "dir", [18.5, 46.6, 5.4, 7.2], "chip"),
    zone("dpad.left", "←", "dir", [12.9, 39.2, 5.4, 7.2], "chip"),
    zone("dpad.right", "→", "dir", [23.9, 39.2, 5.4, 7.2], "chip"),
    // Right stick: R3 hub + ring wedges.
    zone("rthumb", "R3", "hub", [66.1, 56.8, 8.0, 10.0], "round"),
    zone("ry.max", "↑", "dir", [66.1, 48.8, 7.0, 6.0], "chip"),
    zone("ry.min", "↓", "dir", [66.1, 64.8, 7.0, 6.0], "chip"),
    zone("rx.min", "←", "dir", [59.35, 56.8, 5.5, 7.0], "chip"),
    zone("rx.max", "→", "dir", [72.85, 56.8, 5.5, 7.0], "chip"),
];

pub(crate) fn zones_for(persona: &str) -> &'static [Zone; 25] {
    if art_for(persona) == crate::render::ART_DS4 {
        &ZONE_DS4
    } else {
        &ZONE_XBOX
    }
}

/// The selected slot: the payload's `selected` number if it exists, else the
/// first slot.
fn selected_slot(payload: &MapPayload) -> Option<&MapperSlot> {
    payload
        .mapper
        .slots
        .iter()
        .find(|s| s.number == payload.selected)
        .or_else(|| payload.mapper.slots.first())
}

/// Every key bound to `function`, in file order. The inert "None" placeholder
/// was already filtered by the provider, so an empty list IS unbound.
///
/// v10: this — not a joined string — is the unit the mapper works in. MANY
/// KEYS → ONE CONTROL is native to the engine and to the TOML
/// (docs/INPUT-TRANSFORMS.md §1a: `A = ["S", "Enter"]`, press either), and
/// Victor's own imported preset uses it; the page had been folding the list
/// into one tag and the writer had been replacing it.
fn keys_of(slot: &MapperSlot, function: &str) -> Vec<String> {
    slot.bindings.get(function).cloned().unwrap_or_default()
}

/// The separator between a control's keys, on both readers. A MIDDOT, never
/// `+`: `S+Enter` reads as the chord it is not (a chord is `--when`, §1b) —
/// these keys are alternatives, either one presses the control.
const KEY_SEP: &str = " · ";

/// "G", "S · Enter", or "—" for unbound — every key, for the tooltip/aria
/// text and the legend's own reading.
fn key_tag(slot: &MapperSlot, function: &str) -> String {
    let keys = keys_of(slot, function);
    if keys.is_empty() {
        "—".to_owned()
    } else {
        keys.join(KEY_SEP)
    }
}

/// The ON-ART tag: the first key, plus `+N` for the ones that do not fit. The
/// zone is a few millimetres of controller drawing — the legend below carries
/// the full set, and so does the zone's title/aria text.
fn zone_tag(keys: &[String]) -> String {
    match keys {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} +{}", rest.len()),
    }
}

/// How many key chips a legend row draws before it summarizes the tail. Same
/// budget as [`SHARE_MAX`], and the same reason: a row that grows without
/// bound stops being scannable.
const KEY_CHIPS: usize = 3;

/// The note that turns two key tags into a fact: they are ALTERNATIVES.
/// Without it "G · J" is just as readable as "both at once", which is the
/// chord semantics this is not.
fn either_note(count: usize) -> String {
    if count > 1 {
        format!(" ({count} keys — any one of them presses it)")
    } else {
        String::new()
    }
}

/// A zone's tooltip and aria-label: the control, EVERY key on it (the art
/// itself only had room for the first), whether they are alternatives, and
/// which other controls the same key drives.
fn zone_title(function: &str, keys: &[String], tag: &str, share: &[String]) -> String {
    let mut title = format!("{function} — {tag}{}", either_note(keys.len()));
    if !share.is_empty() {
        title.push_str(&format!(" ({})", share_title(tag, share)));
    }
    title
}

/// How many co-bound controls a shared-key badge names before it summarizes.
const SHARE_MAX: usize = 3;

/// Mirrors MapIsland.ts `sharedLabels`: for every zone (table order), the
/// LABELS of the other controls in this preset bound to the same key.
///
/// This is the whole of FEATURE 3's data model. A key bound twice is not a
/// conflict here — the engine applies both (docs/INPUT-TRANSFORMS.md §1a) — so
/// the page's job is to name what a key drives, not to complain about it.
/// Unbound controls (`—`) never "share" anything.
/// v10: two controls share when their key SETS INTERSECT — one key in common
/// is one key that drives both, whether or not either control has others. (It
/// used to compare the joined tags, which quietly stopped noticing the moment
/// a control held more than one key.)
fn shared_labels(slot: &MapperSlot) -> Vec<Vec<String>> {
    let zones = zones_for(&slot.persona);
    let keys: Vec<Vec<String>> = zones.iter().map(|z| keys_of(slot, z.fn_name)).collect();
    keys.iter()
        .enumerate()
        .map(|(i, mine)| {
            if mine.is_empty() {
                return Vec::new();
            }
            zones
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i && keys[*j].iter().any(|k| mine.iter().any(|m| m == k)))
                .map(|(_, z)| legend_label(z))
                .collect()
        })
        .collect()
}

/// "also A · B" — the legend's compact shared-key badge, capped so one key
/// driving eight controls cannot blow the row apart. Empty = not shared (the
/// CSS hides it through the row's `l-shared` class, never `:empty`, which
/// cannot work on an SSR text slot).
fn share_text(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown: Vec<&str> = names.iter().take(SHARE_MAX).map(String::as_str).collect();
    let mut text = format!("also {}", shown.join(" · "));
    if names.len() > SHARE_MAX {
        text.push_str(&format!(" +{}", names.len() - SHARE_MAX));
    }
    text
}

/// The badge's tooltip — the full list, plus the key that does the driving.
fn share_title(key: &str, names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    format!("{key} also drives {}", names.join(", "))
}

/// Mirrors MapIsland.ts `zoneRows`. Every derived string the client also
/// derives; the item SHAPE is client contract. The client may append the
/// hover class (`z-hot`) and the multi-select class (`z-sel`) — SSR has
/// neither hover nor a selection, so the server never emits them.
///
/// `live` is [`learnable`]: when false the zone carries `z-dead`, which is a
/// VISIBLY disabled look (dimmed, `cursor: not-allowed`) and deliberately NOT
/// the `disabled` attribute — a disabled button swallows its own click, and a
/// click on a control that cannot be learned must still say why (FIX 1: never
/// a no-op).
fn zone_rows(slot: &MapperSlot, live: bool) -> SlotValue {
    let shared = shared_labels(slot);
    SlotValue::array(
        zones_for(&slot.persona)
            .iter()
            .zip(shared)
            .map(|(z, share)| {
                let keys = keys_of(slot, z.fn_name);
                let key = key_tag(slot, z.fn_name);
                // z-unbound hides the tag pill via CSS (`:empty` cannot: the
                // SSR text slot leaves marker nodes inside the span).
                let unbound = if key == "—" { " z-unbound" } else { "" };
                let dead = if live { "" } else { " z-dead" };
                let shared_cls = if share.is_empty() { "" } else { " z-shared" };
                SlotValue::object(vec![
                    ("fn".to_owned(), SlotValue::Text(z.fn_name.to_owned())),
                    (
                        "cls".to_owned(),
                        SlotValue::Text(format!("zone z-{}{unbound}{dead}{shared_cls}", z.kind)),
                    ),
                    // FEATURE 1: the control's own name, drawn on the art.
                    ("id".to_owned(), SlotValue::Text(z.label.to_owned())),
                    (
                        "idcls".to_owned(),
                        SlotValue::Text(format!("zid id-{}", z.idk)),
                    ),
                    (
                        "style".to_owned(),
                        SlotValue::Text(format!(
                            "left:{:.1}%;top:{:.1}%;width:{:.1}%;height:{:.1}%",
                            z.cx - z.w / 2.0,
                            z.cy - z.h / 2.0,
                            z.w,
                            z.h
                        )),
                    ),
                    (
                        "title".to_owned(),
                        SlotValue::Text(zone_title(z.fn_name, &keys, &key, &share)),
                    ),
                    // The art shows the first key and counts the rest; the
                    // title above and the legend below name every one.
                    ("tag".to_owned(), SlotValue::Text(zone_tag(&keys))),
                ])
            })
            .collect(),
    )
}

/// Mirrors MapIsland.ts `legendGroup`: the stick/dpad glyph groups need a
/// prefix to stay unambiguous in a flat list ("LS ▲" vs "D-pad ▲"); every
/// other control is named by its identity alone (A vs ✕).
fn legend_group(z: &Zone) -> &'static str {
    if z.fn_name.starts_with("lx.") || z.fn_name.starts_with("ly.") {
        "LS "
    } else if z.fn_name.starts_with("rx.") || z.fn_name.starts_with("ry.") {
        "RS "
    } else if z.fn_name.starts_with("dpad.") {
        "D-pad "
    } else {
        ""
    }
}

/// Group + identity as one string — the row's tooltip/aria text and what a
/// shared-key badge calls a co-bound control.
fn legend_label(z: &Zone) -> String {
    format!("{}{}", legend_group(z), z.label)
}

/// Mirrors MapIsland.ts `legendRowsFor`: the bindings legend below the
/// stage — one row per mappable function, unbound rendered as the honest
/// "—" with the `l-unbound` class. Same client-may-append-hover rule as
/// [`zone_rows`] (`l-hot`, `l-sel`).
///
/// v7: the row leads with the same identity glyph the art now wears (so the
/// two readers are visibly the same control) and ends with the shared-key
/// badge — FEATURE 3's "P also drives A · B", stated as information.
///
/// v9: it also carries the row's own no-JS `<form>` fields. `write` is
/// [`writable`], deliberately wider than `live` — see there.
fn legend_rows(slot: &MapperSlot, live: bool, write: bool) -> SlotValue {
    let shared = shared_labels(slot);
    SlotValue::array(
        zones_for(&slot.persona)
            .iter()
            .zip(shared)
            .map(|(z, share)| {
                let keys = keys_of(slot, z.fn_name);
                let key = key_tag(slot, z.fn_name);
                let unbound = key == "—";
                let mut cls = String::from("lrow");
                if unbound {
                    cls.push_str(" l-unbound");
                }
                if !live {
                    cls.push_str(" l-dead");
                }
                if !share.is_empty() {
                    cls.push_str(" l-shared");
                }
                if keys.len() > 1 {
                    cls.push_str(" l-multi");
                }
                let mut fields: Vec<(String, SlotValue)> = vec![
                    ("fn".to_owned(), SlotValue::Text(z.fn_name.to_owned())),
                    ("label".to_owned(), SlotValue::Text(legend_label(z))),
                    ("id".to_owned(), SlotValue::Text(z.label.to_owned())),
                    (
                        "idcls".to_owned(),
                        SlotValue::Text(format!("lid id-{}", z.idk)),
                    ),
                    (
                        "group".to_owned(),
                        SlotValue::Text(legend_group(z).to_owned()),
                    ),
                    ("key".to_owned(), SlotValue::Text(key.clone())),
                    ("cls".to_owned(), SlotValue::Text(cls)),
                    ("share".to_owned(), SlotValue::Text(share_text(&share))),
                    (
                        "sharetitle".to_owned(),
                        SlotValue::Text(share_title(&key, &share)),
                    ),
                    (
                        "title".to_owned(),
                        SlotValue::Text(format!(
                            "{} — {}{}",
                            z.fn_name,
                            key,
                            either_note(keys.len())
                        )),
                    ),
                    // The ✕ accelerator: only offered where clearing would
                    // actually do something (a bound function on a live page).
                    // Empty string = the CSS hides it, so the row layout does
                    // not jump between states. This one clears the CONTROL —
                    // every key at once; the per-key ✕ chips below take one.
                    (
                        "clear".to_owned(),
                        SlotValue::Text(if live && !unbound { "✕" } else { "" }.to_owned()),
                    ),
                    (
                        "cleartitle".to_owned(),
                        SlotValue::Text(if keys.len() > 1 {
                            format!("clear {} (all {} keys)", z.fn_name, keys.len())
                        } else {
                            format!("clear {}", z.fn_name)
                        }),
                    ),
                    // v9's no-JS row form. The slot NUMBER travels, never the
                    // preset name: the server resolves one from the other, so
                    // a form body can only ever name a slot this cabinet has.
                    ("slot".to_owned(), SlotValue::Text(slot.number.to_string())),
                    (
                        "bindcls".to_owned(),
                        SlotValue::Text(
                            if write {
                                "lbind nojs"
                            } else {
                                "lbind nojs off"
                            }
                            .to_owned(),
                        ),
                    ),
                    (
                        "bindtitle".to_owned(),
                        SlotValue::Text(format!("bind {} ({})", z.fn_name, legend_label(z))),
                    ),
                    // v10's two extra no-JS verbs on the same row form: the
                    // picked key is ADDED to the control's list, or REMOVED
                    // from it, instead of replacing the whole binding.
                    (
                        "addtitle".to_owned(),
                        SlotValue::Text(format!(
                            "add the picked key to {} — it keeps {}",
                            z.fn_name,
                            if unbound { "nothing yet" } else { key.as_str() }
                        )),
                    ),
                    (
                        "rmtitle".to_owned(),
                        SlotValue::Text(format!(
                            "remove just the picked key from {} ({})",
                            z.fn_name, key
                        )),
                    ),
                    // AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3). The badge says
                    // what the game will actually SEE, not what was typed: a
                    // press and a release must each survive a 60 Hz poll, so a
                    // rate above ~15 Hz cannot be delivered however it is
                    // spelled. Empty string = no turbo, and CSS hides the badge
                    // rather than the row changing shape.
                    (
                        "turbo".to_owned(),
                        SlotValue::Text(turbo_tag(slot, z.fn_name)),
                    ),
                    (
                        "turbotitle".to_owned(),
                        SlotValue::Text(turbo_title(slot, z.fn_name)),
                    ),
                    (
                        "turboval".to_owned(),
                        SlotValue::Text(
                            slot.turbo
                                .get(z.fn_name)
                                .map_or_else(String::new, u32::to_string),
                        ),
                    ),
                ];
                fields.extend(key_chip_fields(z.fn_name, &keys, live));
                SlotValue::object(fields)
            })
            .collect(),
    )
}

/// The auto-fire badge on a legend row: what the control will really do.
///
/// The EFFECTIVE rate, not the authored one. `turbo_hz = 30` is a legal thing
/// to write and an impossible thing to deliver — one cycle is a press AND a
/// release, a 60 Hz poller resolves each half no faster than
/// [`MIN_STEP_MS`] — so a badge echoing "30 Hz" back would be the page lying
/// about the file on the file's behalf.
fn turbo_tag(slot: &MapperSlot, function: &str) -> String {
    match slot.turbo.get(function) {
        None => String::new(),
        Some(&hz) => {
            let effective = effective_turbo_hz(hz);
            if effective == hz {
                format!("turbo {hz} Hz")
            } else {
                format!("turbo ~{effective} Hz")
            }
        }
    }
}

fn turbo_title(slot: &MapperSlot, function: &str) -> String {
    match slot.turbo.get(function) {
        None => format!(
            "{function} does not auto-fire — hold its key and it stays down. \"Turbo\" in the \
             learn dialog (or the box in this row without JavaScript) gives it a rate."
        ),
        Some(&hz) => {
            let effective = effective_turbo_hz(hz);
            let mut line = format!(
                "{function} AUTO-FIRES while any of its keys is held: {} ms pressed, {} ms \
                 released, one clock however many keys point at it.",
                turbo_on_ms(hz),
                turbo_off_ms(hz)
            );
            if effective != hz {
                line.push_str(&format!(
                    " The file asks for {hz} Hz and gets about {effective} Hz: a press AND a \
                     release must each survive a 60 Hz poll ({MIN_STEP_MS} ms), so ~15 Hz is the \
                     fastest anything can be delivered."
                ));
            }
            line
        }
    }
}

/// Per-binding turbo halves. Mirrors `ksx_core::TurboBinding`, which is the
/// arithmetic the engine actually runs; pinned against it in the tests below.
fn turbo_on_ms(hz: u32) -> u32 {
    let hz = hz.clamp(1, TURBO_MAX_HZ);
    (1_000 / hz).div_ceil(2).max(MIN_STEP_MS)
}

fn turbo_off_ms(hz: u32) -> u32 {
    let hz = hz.clamp(1, TURBO_MAX_HZ);
    (1_000 / hz)
        .saturating_sub(turbo_on_ms(hz))
        .max(MIN_STEP_MS)
}

fn effective_turbo_hz(hz: u32) -> u32 {
    let cycle = turbo_on_ms(hz) + turbo_off_ms(hz);
    (1_000 + cycle / 2) / cycle
}

/// The legend row's per-key chips — FEATURE: **remove ONE key**, leaving the
/// others alone.
///
/// [`KEY_CHIPS`] fixed slots, not a nested list: the compiled item body has no
/// inner `createList` seam (dogfood ledger #17's neighbour — a list inside a
/// list item is not expressible), so the shape is fixed fields and the tail is
/// summarized like [`share_text`] does. Every chip carries its own remove
/// payload `function|KEY`, which is what the client's `data-rmkey` delegation
/// parses; the row's title still names every key, chipped or not.
///
/// `cls` is how a chip disappears — `:empty` cannot work on an SSR text slot
/// (the marker comments live inside the node), so absence is a class string,
/// exactly like the toast stack's Undo button (ledger #15).
fn key_chip_fields(function: &str, keys: &[String], live: bool) -> Vec<(String, SlotValue)> {
    let mut fields = Vec::new();
    for i in 0..KEY_CHIPS {
        let key = keys.get(i);
        fields.push((
            format!("k{}", i + 1),
            SlotValue::Text(key.cloned().unwrap_or_default()),
        ));
        // `lk1` is what right-aligns the group (see studio.css): only the
        // first chip may take the row's free space.
        let first = if i == 0 { " lk1" } else { "" };
        fields.push((
            format!("k{}cls", i + 1),
            SlotValue::Text(if key.is_some() {
                format!("lkc{first}")
            } else {
                format!("lkc{first} off")
            }),
        ));
        // The ✕ is a SIBLING of the key tag, not the tag itself: clicking a
        // key must never be what deletes it. A chip whose page cannot write is
        // still a chip — the key is the truth, only the accelerator goes.
        fields.push((
            format!("k{}xcls", i + 1),
            SlotValue::Text(
                if key.is_some() && live {
                    "lkx"
                } else {
                    "lkx off"
                }
                .to_owned(),
            ),
        ));
        fields.push((
            format!("k{}rm", i + 1),
            SlotValue::Text(match key {
                Some(key) => format!("{function}|{key}"),
                None => String::new(),
            }),
        ));
        fields.push((
            format!("k{}title", i + 1),
            SlotValue::Text(match key {
                Some(key) if keys.len() > 1 => format!(
                    "remove {key} from {function} — it keeps {}",
                    keys.iter()
                        .filter(|k| *k != key)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(KEY_SEP)
                ),
                Some(key) => format!("remove {key} from {function} — it is the only key"),
                None => String::new(),
            }),
        ));
    }
    let extra = keys.len().saturating_sub(KEY_CHIPS);
    fields.push((
        "kmore".to_owned(),
        SlotValue::Text(if extra > 0 {
            format!("+{extra}")
        } else {
            String::new()
        }),
    ));
    fields.push((
        "kmorecls".to_owned(),
        SlotValue::Text(if extra > 0 { "lkmore" } else { "lkmore off" }.to_owned()),
    ));
    fields.push((
        "kmoretitle".to_owned(),
        SlotValue::Text(if extra > 0 {
            format!("{} more key(s): {}", extra, keys.join(KEY_SEP))
        } else {
            String::new()
        }),
    ));
    fields
}

fn slot_tabs(payload: &MapPayload, selected: Option<&MapperSlot>) -> SlotValue {
    SlotValue::array(
        payload
            .mapper
            .slots
            .iter()
            .map(|s| {
                let active = selected.is_some_and(|sel| sel.number == s.number);
                SlotValue::object(vec![
                    ("num".to_owned(), SlotValue::Text(s.number.to_string())),
                    (
                        "label".to_owned(),
                        SlotValue::Text(format!("P{} · {}", s.number, s.preset)),
                    ),
                    // v9: the tab is an anchor, so slot switching is a plain
                    // GET when there is no JavaScript to intercept it.
                    (
                        "href".to_owned(),
                        SlotValue::Text(format!("/map?slot={}", s.number)),
                    ),
                    (
                        "cls".to_owned(),
                        SlotValue::Text(if active { "tab active" } else { "tab" }.to_owned()),
                    ),
                    // v14: the management table's columns. Same array, second
                    // reader — no new payload, no new verb.
                    (
                        "player".to_owned(),
                        SlotValue::Text(format!("P{}", s.number)),
                    ),
                    ("preset".to_owned(), SlotValue::Text(s.preset.clone())),
                    ("pad".to_owned(), SlotValue::Text(s.persona_label.clone())),
                    ("kbd".to_owned(), SlotValue::Text(s.keyboard.clone())),
                    (
                        "rowcls".to_owned(),
                        SlotValue::Text(if active { "strow on" } else { "strow" }.to_owned()),
                    ),
                ])
            })
            .collect(),
    )
}

/// Can the mapper actually record right now? Needs a reachable daemon, no
/// running session (captured keys never reach the observer), and a daemon
/// that knows the learn verbs at all.
fn learnable(payload: &MapPayload) -> bool {
    payload.session.reachable && !payload.session.running && payload.learn.state != "unavailable"
}

/// Can a binding be WRITTEN right now? Deliberately wider than [`learnable`]:
/// learning needs the panel's keys to reach the daemon's listener (so it
/// refuses while a session captures them, and needs a daemon new enough to
/// have the verbs at all), while writing needs only a daemon — a running
/// session takes a binding change hot (`apply_bindings`, no unplugged pads),
/// and the `map` verb predates the learn verbs.
///
/// This is what gates v9's no-JS forms, which PICK a key from the vocabulary
/// instead of listening for one. Mirrored in MapIsland.ts as `canWrite`.
fn writable(payload: &MapPayload) -> bool {
    payload.session.reachable
}

/// The read-only reason — one honest sentence, worst problem first. Empty
/// when the mapper is live.
fn reason_line(payload: &MapPayload) -> String {
    if payload.mapper.slots.is_empty() {
        return format!("nothing to map — {}", payload.mapper.source);
    }
    if !payload.session.reachable {
        return "read-only: no daemon control channel — start the daemon (tray, or `ksx daemon`), \
                or bind from a shell with the command below"
            .to_owned();
    }
    if payload.session.running {
        return "read-only while emulation runs: the panel's keys are captured, so ksx cannot \
                hear them for mapping. Use \"Pause emulation & map\" above, or bind from a \
                shell with the command below"
            .to_owned();
    }
    if payload.learn.state == "unavailable" {
        return format!(
            "read-only: the daemon does not answer the learn verbs ({}) — restart it on the \
             current ksx build, or bind from a shell with the command below",
            payload
                .learn
                .error
                .as_deref()
                .unwrap_or("no reason reported")
        );
    }
    String::new()
}

/// The prefilled CLI fallback for the selected slot (placeholders for what a
/// click/keypress would fill).
fn cli_line(slot: Option<&MapperSlot>) -> String {
    match slot {
        Some(slot) => format!(
            "ksx map --preset \"{}\" --function <FUNCTION> --key <KEY>",
            slot.preset
        ),
        None => "ksx map --preset <NAME> --function <FUNCTION> --key <KEY>".to_owned(),
    }
}

/// The third restore button's label — the timestamp is the whole point, so it
/// is IN the label rather than hidden in a tooltip.
fn backup_line(selected: Option<&MapperSlot>) -> String {
    match selected.and_then(|s| s.backup.as_deref()) {
        Some(label) => format!("Restore backup from {label}"),
        None => "Restore backup".to_owned(),
    }
}

/// The multi-select toggle's resting class/label. Mirrored in MapIsland.ts,
/// which flips them to `… on` / "Selecting — tap controls" while the mode is
/// live (a class string, never a show — ledger #13).
const SEL_TOGGLE_OFF: &str = "btn btn-row seltoggle";
const SEL_TOGGLE_LABEL_OFF: &str = "Select multiple";

// ── v11/v12: THE MACRO EDITOR — the piano roll, and it SAVES ───────────────
// docs/INPUT-TRANSFORMS.md §6.2, adopted from TAStudio: "rows = steps,
// columns = the slot's controls, cells = held or not". That beats a form with
// an "add step" button because a timed sequence is a SHAPE — you have to see
// ↓, ↘, → as three rows with the diagonal overlapping to know you wrote a
// quarter-circle rather than three unrelated presses.
//
// v11 shipped this READ-ONLY, when §1c's "authoring the sequence itself stays
// TOML-only" was still true and the only output was a block to paste. It is
// not true any more: the daemon grew `map-macro`, which takes ONE WHOLE
// `[macros.<name>]` table ([`crate::control::ControlSource::save_macro`],
// `POST /api/macro/save`, `ksx macro`). So v12 wires the card to it — New,
// Save, Rename and Delete are real writes through that one verb — and the
// TOML block is demoted to a collapsed "copy for sharing" detail.
//
// The SAVE MODEL is explicit-Save, not save-per-edit (the rationale is in
// MapIsland.ts, where the buttons live: a macro write takes a backup and
// hot-swaps the sequence into the running session, so autosaving every painted
// cell would publish half-authored sequences and litter backups). Everything
// that WRITES is JavaScript-only, so the SSR paint below renders the same card
// in its read state — plus the honest note that says which of the two it is.
//
// Every derivation below is mirrored in MapIsland.ts (server derives the SSR
// paint, the client re-derives per edit and per poll), the established rule
// for this page.

/// The shortest step a 60 Hz poller can be relied on to see, in ms.
///
/// A MIRROR of `ksx_core::MIN_STEP_MS` (§0.2: ~16.7 ms per sample, so ~33 ms
/// is two of them). This crate depends on no other ksx crate at runtime, so
/// the number is repeated here and pinned against the real one by
/// `the_sampling_floor_matches_ksx_core`.
pub(crate) const MIN_STEP_MS: u32 = 33;

/// The same floor counted the way a FRAME author counts it: two 60 Hz samples.
/// `frames_ms(2)` is exactly [`MIN_STEP_MS`], which is the point — a warning
/// about a `frames = 1` step that answers in milliseconds is asking its reader
/// to do the conversion that got them there. Pinned in
/// `the_sampling_floor_and_frame_maths_match_ksx_core`.
pub(crate) const MIN_STEP_FRAMES: u32 = 2;

/// The fastest turbo a file may ASK for, and a MIRROR of
/// `ksx_core::TURBO_MAX_HZ` for the same reason as above: one cycle is a press
/// AND a release, so a 60 Hz poll resolves at most 30 of them a second.
/// (The rate a preset actually GETS is lower still — each half is floored at
/// [`MIN_STEP_MS`] — which is exactly what this page has to say out loud.)
pub(crate) const TURBO_MAX_HZ: u32 = 30;

/// 60 Hz frames → ms, rounded to nearest ONCE — `ksx_core::StepDuration::ms`.
/// Rounded once so three frames is 50 ms and not 3 × 17 = 51.
fn frames_ms(frames: u32) -> u32 {
    (frames.saturating_mul(1000) + 30) / 60
}

/// The duration a step ASKS for, in ms. `None` when the file says both units
/// or neither — which is a fault, not a number to guess (`MacroStepFile::
/// duration`), and is reported as one.
fn requested_ms(step: &MacroStepView) -> Option<u32> {
    match (step.ms, step.frames) {
        (Some(ms), None) => Some(ms),
        (None, Some(frames)) => Some(frames_ms(frames)),
        _ => None,
    }
}

/// What the engine would actually hold this step for: below the floor is
/// RAISED unless the author opted out (`MacroStep::effective_ms`).
fn effective_ms(step: &MacroStepView) -> u32 {
    match requested_ms(step) {
        Some(ms) if step.allow_short || ms >= MIN_STEP_MS => ms,
        Some(_) => MIN_STEP_MS,
        None => 0,
    }
}

/// "50 ms" / "3 fr · 50 ms" / "—" — the row's own duration, in the unit it was
/// authored in (a sequence written in frames must still read in frames).
fn duration_text(step: &MacroStepView) -> String {
    match (step.ms, step.frames) {
        (Some(ms), None) => format!("{ms} ms"),
        (None, Some(frames)) => format!("{frames} fr · {} ms", frames_ms(frames)),
        _ => "—".to_owned(),
    }
}

/// Is this step below the sampling floor at all? (Both spellings, one rule.)
fn step_is_short(step: &MacroStepView) -> bool {
    requested_ms(step).is_some_and(|ms| ms < MIN_STEP_MS)
}

/// The number in the row's own duration box, in the unit the FILE spells this
/// step in. Mirrors MapIsland.ts's `macroRowsFor` / `unitOfStep`, with the one
/// difference the client alone can have: the client also remembers a unit the
/// AUTHOR picked for a step whose value has not been retyped yet, and an SSR
/// paint has no author to remember.
fn dur_value(step: &MacroStepView) -> String {
    match (step.ms, step.frames) {
        (_, Some(frames)) if step.ms.is_none() => frames.to_string(),
        (Some(ms), _) => ms.to_string(),
        // Both or neither — the row's amber flag already says so; the box
        // shows the default a new step would take rather than a blank that
        // would write nothing.
        _ => "50".to_owned(),
    }
}

/// `ms` / `fr` — the unit TOGGLE's label. A two-state button, not a `<select>`:
/// a select inside a list item cannot be given its value by an attribute
/// binding (map.ts would have to write every one of them by hand after every
/// poll), and a button's label is its own readout.
fn unit_tag(step: &MacroStepView) -> String {
    if step.frames.is_some() && step.ms.is_none() {
        "fr".to_owned()
    } else {
        "ms".to_owned()
    }
}

fn unit_title(step: &MacroStepView, i: usize) -> String {
    if unit_tag(step) == "fr" {
        format!(
            "step {} is authored in FRAMES — click to switch it to ms (the length is converted, \
             never reinterpreted)",
            i + 1
        )
    } else {
        format!(
            "step {} is authored in MILLISECONDS — click to switch it to frames (the length is \
             converted, never reinterpreted; ksx counts frames at 60 Hz)",
            i + 1
        )
    }
}

/// The INLINE amber flag — short enough to always fit on the row beside the
/// duration, because a truncated warning is a warning nobody reads. The rule
/// it is short for is stated once, in full, in the card's own note; the whole
/// sentence rides the row's `title` ([`step_warning_long`]).
fn step_warning(step: &MacroStepView) -> String {
    match (step.ms, step.frames) {
        (Some(_), Some(_)) => "two units".to_owned(),
        (None, None) => "no duration".to_owned(),
        _ => {
            if !step_is_short(step) {
                return String::new();
            }
            // IN THE AUTHOR'S OWN UNIT. A `frames = 1` step used to be told
            // "16 ms — raised to 33 ms", which is a true sentence about a
            // number the author never typed: it hands them the conversion
            // instead of the answer.
            if let Some(f) = step.frames {
                return if step.allow_short {
                    format!("{f} fr — may be missed")
                } else {
                    format!("{f} fr — raised to {MIN_STEP_FRAMES} fr")
                };
            }
            let ms = step.ms.unwrap_or(0);
            if step.allow_short {
                format!("{ms} ms — may be missed")
            } else {
                format!("{ms} ms — raised to {MIN_STEP_MS} ms")
            }
        }
    }
}

/// The same flag, in full — never a silent acceptance and never a silent
/// rewrite (§1c "the sampling rule, enforced": both outcomes are advisories,
/// and neither is ever quiet). Empty = nothing to say.
fn step_warning_long(step: &MacroStepView) -> String {
    match (step.ms, step.frames) {
        (Some(_), Some(_)) => {
            "says both ms and frames — exactly one, or the file is refused".to_owned()
        }
        (None, None) => {
            "no duration — give it ms or frames (a step with none is refused)".to_owned()
        }
        _ => {
            if !step_is_short(step) {
                return String::new();
            }
            if let Some(f) = step.frames {
                let plural = if f == 1 { "" } else { "s" };
                let each = format!(
                    "{f} frame{plural} is shorter than the {MIN_STEP_FRAMES}-frame floor \
                     ({MIN_STEP_MS} ms — a press has to survive two 60 Hz polls)"
                );
                return if step.allow_short {
                    format!(
                        "{each} — allow_short is on, so it runs as written and the game may \
                         never see it"
                    )
                } else {
                    format!(
                        "{each} — the game may never see it, so ksx raises this step to \
                         {MIN_STEP_FRAMES} frames ({MIN_STEP_MS} ms)"
                    )
                };
            }
            let ms = step.ms.unwrap_or(0);
            if step.allow_short {
                format!(
                    "{ms} ms is shorter than ~2 poll intervals ({MIN_STEP_MS} ms) — allow_short \
                     is on, so it runs as written and the game may never see it"
                )
            } else {
                format!(
                    "{ms} ms is shorter than ~2 poll intervals ({MIN_STEP_MS} ms) — the game may \
                     never see it, so ksx raises this step to {MIN_STEP_MS} ms"
                )
            }
        }
    }
}

/// The sampling rule, stated ONCE, where the amber rows can point at it (§0.2).
/// The per-row flag is short so it always fits; this is what it means.
pub(crate) const MACRO_RULE_LINE: &str =
    "Amber steps are shorter than ~2 poll intervals — 33 ms, or 2 frames if you are counting \
     frames — which is the shortest thing a game can be relied on to see. A 1-frame step is \
     not unreliable, it is invisible. ksx raises a short step to 33 ms so it lands; a step \
     marked allow_short runs exactly as written and can be missed entirely. Neither is ever \
     silent, and Save asks before it writes either one.";

/// THE RING, stated once, under the grid: what the eight columns of a direction
/// group are, what each is called, and — for the four that are picks rather than
/// stored names — exactly what ksx writes when you tick one.
///
/// The numpad digits live HERE and in the tooltips, never in the glyph row: a
/// second line of digits under only 24 of 37 columns makes the header ragged,
/// and the digit is a lookup key (for somebody who read "3" on Dustloop or typed
/// digits into MAME's `joystick_map`), not a label.
pub(crate) const MACRO_RING_LINE: &str =
    "Each direction group runs ↑ ↖ ← ↙ ↓ ↘ → ↗ (numpad 8 7 4 1 2 3 6 9), so a motion is a \
     SHAPE: a quarter-circle forward is a staircase, a half-circle a straight line, a dragon \
     punch a hook. The four diagonals are picks, not new bindings — ticking ↘ (down-right, \
     numpad 3; a move list spells it d/f, which is only down-FORWARD while you face right — ksx \
     has no idea which way you are facing) stores dpad.down + dpad.right on that step, which is \
     what a diagonal has always been in this file. THERE ARE THREE OF THESE GROUPS — D-PAD, \
     LEFT STICK and RIGHT STICK — and the grid scrolls sideways to reach them, so the one you \
     want may be off the edge; the band above the arrows names whichever you are looking at. \
     Tick the diagonal on the group your preset's own direction keys drive, because a motion \
     written on the other one is published faithfully and read by nobody. Each row spells the \
     pair it wrote beside its name.";

/// A macro's run length at the durations the engine will use.
fn total_ms(mac: &MacroView) -> u32 {
    mac.steps.iter().map(effective_ms).sum()
}

/// The macro the page paints: the payload's `macro_selected` if the preset has
/// it (case-insensitively, like every function name), else the first one —
/// and `None` when the preset holds no macros at all.
///
/// v12 removed the "starter draft" fallback that used to fill this. It minted
/// a macro called `my-macro` that existed ONLY in the browser, so the card
/// offered to bind a trigger to it and the daemon answered — correctly —
/// `preset "IPAC P1" defines no macro called "my-macro"`. A name on this card
/// is now always a name the preset holds; the way to get a new one is the
/// card's own "＋ New macro", which WRITES it.
fn selected_macro(payload: &MapPayload) -> Option<MacroView> {
    payload
        .macros
        .macros
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&payload.macro_selected))
        .or_else(|| payload.macros.macros.first())
        .cloned()
}

/// The body "＋ New macro" writes: one real 50 ms step at the default
/// policies. A macro with NO steps is refused by the loader and by the daemon
/// (`mapping::macro_body_issues`), so a new table has to arrive with one.
///
/// The BUTTON lives in MapIsland.ts (`newMacroBody`) — creating a macro is a
/// fetch, and this crate renders rather than writes. This mirror exists so the
/// starter body can be pinned against the real loader in a test: a "New macro"
/// the daemon would refuse is a dead button, and nobody would find out until a
/// user pressed it.
#[cfg(test)]
pub(crate) fn new_macro_body(name: &str) -> MacroView {
    MacroView {
        name: name.to_owned(),
        steps: vec![MacroStepView {
            hold: Vec::new(),
            ms: Some(50),
            frames: None,
            allow_short: false,
        }],
        on_release: "finish".to_owned(),
        retrigger: "ignore".to_owned(),
        interrupt: "none".to_owned(),
        // A new macro runs ONCE. Auto-fire is asked for by name, never a
        // default a starter body hands somebody who did not ask.
        repeat: "once".to_owned(),
        turbo_hz: None,
        gap_ms: None,
        triggers: Vec::new(),
        // A new macro RUNS. Nobody creates one switched off.
        disabled: false,
    }
}

// ── v12: the frame arithmetic, on screen ───────────────────────────────────
// Victor asked it directly: "a 60fps frame is only like sixteenth
// milliseconds? maybe we can show that math." So the duration editor prints
// the conversion live, with the sampling floor in the SAME units — which is
// what makes an amber row explain itself instead of citing a rule.
//
// The target rate is DISPLAY-ONLY. Authoring against a game's real rate is
// useful (59.94, 57, 55 are all common on a cabinet), but there is nowhere to
// put one: the preset file's step is hold / ms / frames / allow_short, the
// `map-macro` body ([`crate::control::MacroWrite`]) carries exactly those, and
// `ksx_core::StepDuration::Frames` counts frames at 60 Hz full stop. A field
// the daemon would drop is the silent no-op this page bans — so the selector
// converts for the author and SAYS that `frames = N` still runs at 60 Hz.
// The rate lives client-side; SSR paints the 60 Hz line.

/// The engine's floor in the author's units: "33 ms (2.0 frames @ 60 Hz)".
fn floor_text(rate: f64) -> String {
    format!(
        "{MIN_STEP_MS} ms ({:.1} frames @ {} Hz)",
        (f64::from(MIN_STEP_MS) * rate) / 1000.0,
        rate_text(rate)
    )
}

fn rate_text(rate: f64) -> String {
    if rate.fract() == 0.0 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.2}")
    }
}

/// The live conversion for one step. Mirrored in MapIsland.ts `frameMath`.
fn frame_math(step: Option<&MacroStepView>, rate: f64) -> String {
    let floor = format!(
        "The engine can only see steps of {} or longer.",
        floor_text(rate)
    );
    let Some(step) = step else {
        return format!("Pick a step's ⏱ to retime it. {floor}");
    };
    match (step.ms, step.frames) {
        (Some(_), Some(_)) => format!(
            "This step says both ms and frames — keep exactly one, or the preset will not \
             load. {floor}"
        ),
        (None, None) => format!("This step has no duration — give it ms or frames. {floor}"),
        (None, Some(frames)) => {
            let ksx = f64::from(frames_ms(frames));
            let plural = if frames == 1 { "" } else { "s" };
            if (rate - 60.0).abs() < f64::EPSILON {
                format!("{frames} frame{plural} @ 60 Hz = {ksx:.1} ms. {floor}")
            } else {
                let at_rate = (f64::from(frames) * 1000.0) / rate;
                format!(
                    "{frames} frame{plural} @ {} Hz = {at_rate:.1} ms — but ksx counts frames at \
                     60 Hz, so this step runs {ksx:.1} ms. To match the game, switch the unit to \
                     ms and enter {}. {floor}",
                    rate_text(rate),
                    at_rate.round() as u32
                )
            }
        }
        (Some(ms), None) => format!(
            "{ms} ms = {:.1} frames @ {} Hz. {floor}",
            (f64::from(ms) * rate) / 1000.0,
            rate_text(rate)
        ),
    }
}

/// The rate the SSR paint assumes — the default of the card's own selector.
const SSR_RATE_HZ: f64 = 60.0;

/// `macro.<name>` — the function name the `map` verb takes for a TRIGGER.
/// Same spelling `ksx_config::macro_function_name` writes into the file.
fn macro_function(name: &str) -> String {
    format!("macro.{name}")
}

/// The macro tab strip: one anchor per macro the preset defines, so switching
/// works with JavaScript off (`/map?slot=N&macro=NAME` is a route).
fn macro_tabs(payload: &MapPayload, current: Option<&MacroView>) -> SlotValue {
    SlotValue::array(
        payload
            .macros
            .macros
            .iter()
            .map(|m| {
                let active =
                    current.is_some_and(|current| m.name.eq_ignore_ascii_case(&current.name));
                SlotValue::object(vec![
                    ("name".to_owned(), SlotValue::Text(m.name.clone())),
                    (
                        "label".to_owned(),
                        SlotValue::Text(format!("{} · {} steps", m.name, m.steps.len())),
                    ),
                    (
                        "href".to_owned(),
                        SlotValue::Text(format!(
                            "/map?slot={}&macro={}",
                            payload.selected,
                            urlencode_value(&m.name)
                        )),
                    ),
                    (
                        "cls".to_owned(),
                        SlotValue::Text(if active { "mactab active" } else { "mactab" }.to_owned()),
                    ),
                ])
            })
            .collect(),
    )
}

/// Percent-encode a macro name for the tab's href. Names come from a file and
/// are otherwise unconstrained, so this is not optional.
fn urlencode_value(text: &str) -> String {
    let mut out = String::new();
    let mut utf8 = [0u8; 4];
    for c in text.chars().take(120) {
        for byte in c.encode_utf8(&mut utf8).bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char);
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
    }
    out
}

/// The grid's COLUMN HEADERS: the slot's controls, in pad-reading order, with
/// the same identity glyphs the art and the legend already use — so a column is
/// recognisably the button it is (persona-aware: `A` on Xbox, `✕` on a
/// DualShock) — and every direction group as its eight-position ring.
///
/// The glyph row is ARROWS ONLY, one line, no digits: it is a screen, and
/// screens speak arrows in this genre. The numpad digit lives in the tooltip
/// and the ring line, where it is a lookup key for somebody who read "3" on
/// Dustloop or typed digits into `joystick_map`.
fn macro_cols(slot: Option<&MapperSlot>) -> SlotValue {
    let persona = slot.map_or("xbox360", |s| s.persona.as_str());
    SlotValue::array(
        macro_columns(persona)
            .into_iter()
            .map(|c| {
                SlotValue::object(vec![
                    ("fn".to_owned(), SlotValue::Text(c.token)),
                    ("id".to_owned(), SlotValue::Text(c.glyph)),
                    // UNIFORM colour, deliberately: the grid header carries one
                    // of these per control at column width, and a row of
                    // coloured discs that narrow is noise rather than
                    // information. The identity colours earn their place on the
                    // controller art, where they map to physical buttons, and in
                    // the legend beside it — here the column is NAMED, not
                    // badged. `card`/`diag` are TYPE, not palette: the cardinals
                    // carry the stronger column rule so each block reads as a
                    // compass.
                    ("idcls".to_owned(), SlotValue::Text(c.idcls.to_owned())),
                    ("title".to_owned(), SlotValue::Text(c.title)),
                ])
            })
            .collect(),
    )
}

/// The GROUP BAND above the glyph row — one cell per run of columns belonging
/// to the same part of the pad, spanning it.
///
/// Not decoration: without it the header carries three identical `↑ ↖ ← ↙ ↓ ↘ →
/// ↗` runs told apart only by a tooltip (the same defect the old four-column
/// layout had with `▲ ▼ ◀ ▶`).
fn macro_groups(slot: Option<&MapperSlot>) -> SlotValue {
    let persona = slot.map_or("xbox360", |s| s.persona.as_str());
    let mut runs: Vec<(&'static str, usize)> = Vec::new();
    for column in macro_columns(persona) {
        match runs.last_mut() {
            Some((band, n)) if *band == column.band => *n += 1,
            _ => runs.push((column.band, 1)),
        }
    }
    SlotValue::array(
        runs.into_iter()
            .map(|(band, span)| {
                SlotValue::object(vec![
                    ("label".to_owned(), SlotValue::Text(band.to_owned())),
                    // A CLASS, never an inline `grid-column` — forma's CSP
                    // nonce-locks style-src (ledger #13), and a span of 3/4/8/9
                    // is a closed set anyway.
                    ("cls".to_owned(), SlotValue::Text(format!("macgrp g{span}"))),
                ])
            })
            .collect(),
    )
}

/// How one PRESENTED control is named on this pad.
///
/// A coalesced diagonal (the hat+stick double-binding every in-box template
/// writes) joins its mechanisms with **"and"**, never with `+`. `+` is this
/// row's separator for ANOTHER CONTROL — "D-pad ↘ + A" is two things held at
/// once — so `"D-pad + LS ↘"` read as a control called "D-pad" and a control
/// called "LS ↘", which is two lies in four words: the D-pad is pointing ↘ too,
/// and there is only one control here. It also collided with [`hold_cls`]'s
/// `· together` tail, which that row does not get (it folds to ONE presented
/// control) — so the row holding the MOST bindings looked like the row holding
/// two. "D-pad and LS ↘" is one control, said on two mechanisms, and "and" is
/// the joiner [`macro_motion_line`] already uses for exactly this list.
fn held_label(zones: &[Zone], hold: &[String], held: &Held) -> String {
    match held {
        Held::Diagonal {
            diag, mechanisms, ..
        } => format!(
            "{}{}",
            mechanisms
                .iter()
                .map(|m| m.group().trim_end())
                .collect::<Vec<_>>()
                .join(" and "),
            format_args!(" {}", diag.glyph()),
        ),
        Held::Plain { member } => {
            let f = &hold[*member];
            zones
                .iter()
                .find(|z| z.fn_name.eq_ignore_ascii_case(f))
                .map_or_else(|| f.clone(), legend_label)
        }
    }
}

/// What one step holds, named the way this pad names it: "D-pad ↘ + A".
///
/// The row readout is where the model is TAUGHT. A diagonal reads as one
/// control — because that is what the player picked and what the player means —
/// and [`hold_expand`] says, beside it, exactly which two names the file
/// carries. Nothing is hidden and nothing has to be decoded from lit cells
/// twelve columns apart.
fn hold_text(slot: Option<&MapperSlot>, hold: &[String]) -> String {
    if hold.is_empty() {
        return "(nothing — neutral gap)".to_owned();
    }
    let persona = slot.map_or("xbox360", |s| s.persona.as_str());
    let zones = zones_for(persona);
    fold(hold)
        .iter()
        .map(|h| held_label(zones, hold, h))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// THE LEDGER LINE: every diagonal in this step, spelled as the pair the file
/// actually stores — `↘ = dpad.down + dpad.right`.
///
/// This is what keeps the lens honest. The presentation says "one control"; the
/// storage says "two holds"; this line says both at once, on the row itself, so
/// nobody has to open the TOML to find out what a pick wrote. Empty when the
/// step holds no diagonal.
fn hold_expand(hold: &[String]) -> String {
    fold(hold)
        .iter()
        .filter_map(|h| match h {
            Held::Diagonal { diag, members, .. } => Some(format!(
                "{} = {}",
                diag.glyph(),
                members
                    .iter()
                    .map(|&i| hold[i].as_str())
                    .collect::<Vec<_>>()
                    .join(" + ")
            )),
            Held::Plain { .. } => None,
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The row's hover text: what it holds, how long the engine runs it — and, when
/// a diagonal is on the row, the pair the file stores.
///
/// **Why the ledger is said twice.** The `.macexp` span beside the words is the
/// primary place, but it is the first thing the row bar gives up as the card
/// narrows, and it gives it up SILENTLY: measured on the real page it is 154px
/// at a 1440 viewport, 60px at 1100 and **zero** at 820 — still `display:
/// block`, still in the DOM, hoverable by nobody. An honesty line that
/// disappears without saying so is not an honesty line, and this row is the one
/// place on the card that cannot be truncated. So the title carries it too, and
/// the narrow-width drop of the span becomes a deliberate layout choice rather
/// than the quiet loss of the only statement of what a pick wrote.
fn row_title(slot: Option<&MapperSlot>, step: &MacroStepView, i: usize) -> String {
    let base = format!(
        "step {} holds {} for {} (the engine runs it for {} ms)",
        i + 1,
        hold_text(slot, &step.hold),
        duration_text(step),
        effective_ms(step)
    );
    let expand = hold_expand(&step.hold);
    if expand.is_empty() {
        base
    } else {
        format!("{base} — {expand}")
    }
}

fn hold_expand_cls(hold: &[String]) -> &'static str {
    if hold_expand(hold).is_empty() {
        "macexp off"
    } else {
        "macexp"
    }
}

/// The hold readout's own class, over PRESENTED controls rather than stored
/// ones: a diagonal is one control, so `↓ + →` no longer reads as two.
/// The accent is for a row that really does hold several things at once.
fn hold_cls(hold: &[String]) -> &'static str {
    match fold(hold).len() {
        0 => "machold none",
        1 => "machold",
        _ => "machold both",
    }
}

/// The ROW BAR beside the grid: step number, what it holds IN WORDS, its
/// duration, the amber flag, and the five step verbs. One list, because the
/// row's controls and the row's label are the same row — the matrix beside it
/// aligns on a fixed row height.
///
/// `selected` is the step the duration editor is pointed at (client-only: an
/// SSR paint has selected nothing, so it passes `None`).
fn macro_rows(
    mac: Option<&MacroView>,
    slot: Option<&MapperSlot>,
    selected: Option<usize>,
) -> SlotValue {
    let Some(mac) = mac else {
        return SlotValue::array(Vec::new());
    };
    let last = mac.steps.len().saturating_sub(1);
    let only = mac.steps.len() == 1;
    SlotValue::array(
        mac.steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let warn = step_warning(step);
                let mut cls = String::from("macrow");
                if !warn.is_empty() {
                    cls.push_str(" short");
                }
                if selected == Some(i) {
                    cls.push_str(" sel");
                }
                SlotValue::object(vec![
                    ("n".to_owned(), SlotValue::Text((i + 1).to_string())),
                    ("cls".to_owned(), SlotValue::Text(cls)),
                    ("dur".to_owned(), SlotValue::Text(duration_text(step))),
                    (
                        "durtitle".to_owned(),
                        SlotValue::Text(row_title(slot, step, i)),
                    ),
                    // FIX 2: THE DURATION IS EDITED ON THE ROW. It used to be
                    // one field under the grid pointed at whichever step was
                    // "selected" — so changing a time meant picking a row
                    // first, and anything that dropped the selection (a poll,
                    // a re-seed) silently took the editor with it. Every row
                    // now carries its own number, its own unit toggle and its
                    // own warn class; `durrow` is the index the box writes to,
                    // which is all the state the gesture needs.
                    ("durval".to_owned(), SlotValue::Text(dur_value(step))),
                    ("durrow".to_owned(), SlotValue::Text(i.to_string())),
                    (
                        "durcls".to_owned(),
                        SlotValue::Text(
                            if step_is_short(step) {
                                "macrowdur short"
                            } else {
                                "macrowdur"
                            }
                            .to_owned(),
                        ),
                    ),
                    ("unit".to_owned(), SlotValue::Text(unit_tag(step))),
                    ("unitact".to_owned(), SlotValue::Text(format!("unit|{i}"))),
                    ("unittitle".to_owned(), SlotValue::Text(unit_title(step, i))),
                    (
                        "hold".to_owned(),
                        SlotValue::Text(hold_text(slot, &step.hold)),
                    ),
                    (
                        "holdcls".to_owned(),
                        SlotValue::Text(hold_cls(&step.hold).to_owned()),
                    ),
                    // The ledger: which two names the file carries for each
                    // diagonal on this row. The lens is only honest if the
                    // storage is visible without opening the TOML.
                    ("exp".to_owned(), SlotValue::Text(hold_expand(&step.hold))),
                    (
                        "expcls".to_owned(),
                        SlotValue::Text(hold_expand_cls(&step.hold).to_owned()),
                    ),
                    ("warn".to_owned(), SlotValue::Text(warn.clone())),
                    (
                        "warntitle".to_owned(),
                        SlotValue::Text(step_warning_long(step)),
                    ),
                    (
                        "warncls".to_owned(),
                        SlotValue::Text(
                            if warn.is_empty() {
                                "macwarn off"
                            } else {
                                "macwarn"
                            }
                            .to_owned(),
                        ),
                    ),
                    // The five step verbs, each carrying `verb|index`. Same
                    // delegation shape as the legend's `data-rmkey`.
                    ("selact".to_owned(), SlotValue::Text(format!("sel|{i}"))),
                    ("upact".to_owned(), SlotValue::Text(format!("up|{i}"))),
                    ("dnact".to_owned(), SlotValue::Text(format!("down|{i}"))),
                    ("iaact".to_owned(), SlotValue::Text(format!("insa|{i}"))),
                    ("ibact".to_owned(), SlotValue::Text(format!("insb|{i}"))),
                    ("delact".to_owned(), SlotValue::Text(format!("del|{i}"))),
                    // FIX 1: on the LAST remaining step the ✕ empties the row
                    // instead of removing it, so the editor cannot build the
                    // zero-step macro `mapping::save_macro` refuses — and the
                    // title says which of the two it is about to do, so it
                    // never reads as a delete that did not work.
                    (
                        "deltitle".to_owned(),
                        SlotValue::Text(
                            if only {
                                "clear this step (a macro needs at least one — this empties the \
                                 row instead of removing it, which is a legal neutral gap)"
                            } else {
                                "delete this step"
                            }
                            .to_owned(),
                        ),
                    ),
                    (
                        "upcls".to_owned(),
                        SlotValue::Text(if i == 0 { "macbtn off" } else { "macbtn" }.to_owned()),
                    ),
                    (
                        "dncls".to_owned(),
                        SlotValue::Text(if i == last { "macbtn off" } else { "macbtn" }.to_owned()),
                    ),
                ])
            })
            .collect(),
    )
}

/// What one column says about one step.
enum CellState {
    /// Not held.
    Off,
    /// Held, and this column is the whole of it.
    On,
    /// Held as HALF of a folded diagonal. The diagonal's own cell carries the
    /// filled mark; this one carries a subordinate tick, because pretending the
    /// cardinal is not in the file would be the lens lying about the storage.
    Part(Diag),
}

/// The matrix itself, FLAT: `steps × 37` cells in row-major order, laid out by
/// a 37-column CSS grid.
///
/// Flat because the compiled list item body has no inner `createList` seam
/// (dogfood ledger #17's neighbour — the same constraint that made the legend's
/// key chips fixed fields). One list of `rows × columns` cells and a
/// `grid-template-columns` is the shape that survives it, and it reconciles
/// exactly as well as a nested one would.
///
/// A step holding `ly.min` + `lx.max` lights the LS `↘` cell — not two stray
/// ticks eight columns apart. That is the whole point, and it works on a
/// hand-written step nobody made through this page.
fn macro_cells(
    mac: Option<&MacroView>,
    slot: Option<&MapperSlot>,
    selected: Option<usize>,
) -> SlotValue {
    let Some(mac) = mac else {
        return SlotValue::array(Vec::new());
    };
    let persona = slot.map_or("xbox360", |s| s.persona.as_str());
    let columns = macro_columns(persona);
    let zones = zones_for(persona);
    let mut cells = Vec::with_capacity(mac.steps.len() * columns.len());
    for (i, step) in mac.steps.iter().enumerate() {
        let view = fold(&step.hold);
        // Which diagonal (if any) each held entry was folded into, and which
        // (mechanism, diagonal) pairs are lit.
        let mut member_of: Vec<Option<Diag>> = vec![None; step.hold.len()];
        let mut lit: Vec<(Mechanism, Diag, bool)> = Vec::new();
        for held in &view {
            if let Held::Diagonal {
                diag,
                mechanisms,
                members,
                exact,
            } = held
            {
                for &m in members {
                    member_of[m] = Some(*diag);
                }
                for &mechanism in mechanisms {
                    lit.push((mechanism, *diag, *exact));
                }
            }
        }
        for column in &columns {
            let state = match parse_diag_token(&column.token) {
                Some((mechanism, diag)) => lit
                    .iter()
                    .find(|(m, d, _)| *m == mechanism && *d == diag)
                    .map_or(CellState::Off, |_| CellState::On),
                // A direction column matches by WHERE IT POINTS, not by
                // spelling: `ly.-16384` is the down half of this pad's left
                // stick however the file spells it.
                None => match pointing(&column.token) {
                    Some(want) => step
                        .hold
                        .iter()
                        .position(|f| points_same_way(f, want))
                        .map_or(CellState::Off, |at| match member_of[at] {
                            Some(diag) => CellState::Part(diag),
                            None => CellState::On,
                        }),
                    None => {
                        if step
                            .hold
                            .iter()
                            .any(|f| f.eq_ignore_ascii_case(&column.token))
                        {
                            CellState::On
                        } else {
                            CellState::Off
                        }
                    }
                },
            };
            let approx = matches!(state, CellState::On)
                && lit
                    .iter()
                    .any(|(m, d, exact)| !exact && diag_token(*m, *d) == column.token);
            let mut cls = String::from("maccell");
            match state {
                CellState::On => cls.push_str(" on"),
                CellState::Part(_) => cls.push_str(" part"),
                CellState::Off => {}
            }
            if approx {
                cls.push_str(" approx");
            }
            if column.idcls.ends_with("diag") {
                cls.push_str(" isdiag");
            }
            if selected == Some(i) {
                cls.push_str(" inrow");
            }
            let name = column_name(zones, column);
            let title = match &state {
                CellState::On if approx => format!(
                    "step {} holds {name} — as written, not at full deflection ({})",
                    i + 1,
                    step.hold.join(" + ")
                ),
                CellState::On => format!("step {} holds {name}", i + 1),
                CellState::Part(diag) => format!(
                    "step {} holds {name} as half of {} — the {} column beside it is the pick",
                    i + 1,
                    diag.glyph(),
                    diag.glyph()
                ),
                CellState::Off => format!("step {} does not hold {name}", i + 1),
            };
            cells.push(SlotValue::object(vec![
                ("cls".to_owned(), SlotValue::Text(cls)),
                (
                    "cell".to_owned(),
                    SlotValue::Text(format!("{i}|{}", column.token)),
                ),
                (
                    "mark".to_owned(),
                    SlotValue::Text(
                        match state {
                            CellState::On => "●",
                            CellState::Part(_) => "·",
                            CellState::Off => "",
                        }
                        .to_owned(),
                    ),
                ),
                ("title".to_owned(), SlotValue::Text(title)),
            ]));
        }
    }
    SlotValue::array(cells)
}

/// Does `function` point the same way as `want`, whatever it is spelled?
///
/// `exact` is deliberately NOT compared: `ly.-16384` is the down half of the
/// left stick just as `ly.min` is, and a grid that only lit the canonical
/// spelling would leave a hand-written step looking unheld. Mirrored in
/// MapIsland.ts `pointsSameWay`.
fn points_same_way(function: &str, want: Pointing) -> bool {
    pointing(function).is_some_and(|p| {
        p.mechanism == want.mechanism && p.vertical == want.vertical && p.positive == want.positive
    })
}

/// `diag:<mech>:<diag>` back into its two halves, or `None` for a function name.
fn parse_diag_token(token: &str) -> Option<(Mechanism, Diag)> {
    let rest = token.strip_prefix("diag:")?;
    let (mech, diag) = rest.split_once(':')?;
    let mechanism = Mechanism::ALL.into_iter().find(|m| m.token() == mech)?;
    let diag = Diag::ALL.into_iter().find(|d| d.token() == diag)?;
    Some((mechanism, diag))
}

/// What a column is called in a sentence — "D-pad ↘ (down-right)", "LS ↓",
/// "✕ (A)".
fn column_name(zones: &[Zone], column: &MacroColumn) -> String {
    match parse_diag_token(&column.token) {
        Some((mechanism, diag)) => {
            format!("{}{} ({})", mechanism.group(), diag.glyph(), diag.words())
        }
        None => match Mechanism::of(&column.token) {
            Some(mechanism) => format!("{}{} ({})", mechanism.group(), column.glyph, column.token),
            None => zones
                .iter()
                .find(|z| z.fn_name.eq_ignore_ascii_case(&column.token))
                .map_or_else(
                    || column.token.clone(),
                    |z| format!("{} ({})", legend_label(z), z.fn_name),
                ),
        },
    }
}

/// TOML string escaping — macro names and key names come from a file.
fn toml_str(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The macro as TOML — v12's "advanced / copy for sharing" detail, and the
/// hand-editing path for a page with no JavaScript. Not the save any more:
/// that is the card's own Save button, through the `map-macro` verb.
///
/// Emitted exactly as `ksx_config::MacroFile` spells it — defaults omitted (a
/// macro-free-looking file stays macro-free-looking), the duration in the unit
/// it was authored in, and the trigger row underneath so the macro arrives with
/// the key that starts it. When there is no trigger yet the row is COMMENTED
/// OUT rather than filled with a placeholder, because a pasted
/// `macro.x = "<KEY>"` would not load.
fn macro_toml(mac: &MacroView) -> String {
    let mut out = format!("[macros.{}]\n", mac.name);
    if mac.on_release != "finish" {
        out.push_str(&format!("on_release = {}\n", toml_str(&mac.on_release)));
    }
    if mac.retrigger != "ignore" {
        out.push_str(&format!("retrigger = {}\n", toml_str(&mac.retrigger)));
    }
    if mac.interrupt != "none" {
        out.push_str(&format!("interrupt = {}\n", toml_str(&mac.interrupt)));
    }
    if !mac.repeat.is_empty() && mac.repeat != "once" {
        out.push_str(&format!("repeat = {}\n", toml_str(&mac.repeat)));
    }
    // Two spellings of one number, so exactly ONE is emitted — a block giving
    // both is refused by the loader, and pasting one back must never be how a
    // reader finds that out.
    if let Some(hz) = mac.turbo_hz {
        out.push_str(&format!("turbo_hz = {hz}\n"));
    } else if let Some(ms) = mac.gap_ms {
        out.push_str(&format!("gap_ms = {ms}\n"));
    }
    out.push_str("steps = [\n");
    for step in &mac.steps {
        let hold = step
            .hold
            .iter()
            .map(|f| toml_str(f))
            .collect::<Vec<_>>()
            .join(", ");
        let duration = match (step.ms, step.frames) {
            (Some(ms), None) => format!("ms = {ms}"),
            (None, Some(frames)) => format!("frames = {frames}"),
            (Some(ms), Some(frames)) => format!("ms = {ms}, frames = {frames}"),
            (None, None) => "ms = ".to_owned(),
        };
        out.push_str(&format!("  {{ hold = [{hold}], {duration}"));
        if step.allow_short {
            out.push_str(", allow_short = true");
        }
        out.push_str(" },\n");
    }
    out.push_str("]\n\n[bindings]\n");
    match mac.triggers.as_slice() {
        [] => out.push_str(&format!(
            "# macro.{} = \"<KEY>\"   # no trigger yet — bind one above, or with the line below\n",
            mac.name
        )),
        [one] => out.push_str(&format!("macro.{} = {}\n", mac.name, toml_str(one))),
        many => out.push_str(&format!(
            "macro.{} = [{}]\n",
            mac.name,
            many.iter()
                .map(|k| toml_str(k))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
    out
}

/// The trigger's `ksx map` line — complete rather than a template when a key
/// is already bound, and a template naming no macro when the preset holds
/// none (there is nothing to point a key at yet).
fn macro_cli(preset: &str, mac: Option<&MacroView>) -> String {
    let Some(mac) = mac else {
        return format!("ksx map --preset \"{preset}\" --function macro.<NAME> --key <KEY>");
    };
    let key = mac.triggers.first().map_or("<KEY>", String::as_str);
    format!(
        "ksx map --preset \"{preset}\" --function {} --key {key}",
        macro_function(&mac.name)
    )
}

/// Which keys start this macro, in words.
fn macro_trigger_line(mac: Option<&MacroView>) -> String {
    let Some(mac) = mac else {
        return String::new();
    };
    match mac.triggers.as_slice() {
        [] => "no trigger key yet — nothing starts this macro".to_owned(),
        [one] => format!("started by {one}"),
        many => format!(
            "started by {} — any one of them ({} keys)",
            many.join(KEY_SEP),
            many.len()
        ),
    }
}

// ── FIX 1c: COMMON MOTIONS — which mechanism they write, and why ───────────
// The quarter-circle is where both traps bite at once: its middle step is the
// diagonal (one row, two controls), and a pad has THREE ways to say "right".
// ksx publishes exactly what a step names, so a motion written in dpad holds
// on a preset whose player keys drive the left stick is published faithfully
// and read by nobody — `Issue::MacroHoldsOtherMechanism` in
// ksx-config/src/validate.rs, an advisory that only arrives AFTER a save.
// Generating from the slot's OWN bound direction keys means it never fires.
//
// The buttons themselves are client-only (they edit a draft), but this
// sentence is SSR'd like everything else on the card: a no-JS reader is told
// which mechanism their preset drives, which is the fact, not the affordance.

/// Which control a preset's direction keys drive. Mirror of
/// `ksx_core::socd::DirMechanism` (= `ksx_config::validate::Mechanism`, which
/// is now a re-export of it). Pinned by `the_diagonal_lens_matches_ksx_core`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mechanism {
    Dpad,
    LeftStick,
    RightStick,
}

impl Mechanism {
    /// Canonical order — the order a coalesced diagonal lists its mechanisms in
    /// and the order the grid draws its direction groups in.
    const ALL: [Mechanism; 3] = [Mechanism::Dpad, Mechanism::LeftStick, Mechanism::RightStick];

    fn of(function: &str) -> Option<Self> {
        let f = function.to_ascii_lowercase();
        if f.starts_with("dpad.") {
            Some(Mechanism::Dpad)
        } else if f.starts_with("lx.") || f.starts_with("ly.") {
            Some(Mechanism::LeftStick)
        } else if f.starts_with("rx.") || f.starts_with("ry.") {
            Some(Mechanism::RightStick)
        } else {
            None
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            Mechanism::Dpad => "the dpad",
            Mechanism::LeftStick => "the left stick (lx/ly)",
            Mechanism::RightStick => "the right stick (rx/ry)",
        }
    }

    /// The prefix a flat list needs to keep three identical arrow runs apart —
    /// the same one [`legend_group`] writes.
    const fn group(self) -> &'static str {
        match self {
            Mechanism::Dpad => "D-pad ",
            Mechanism::LeftStick => "LS ",
            Mechanism::RightStick => "RS ",
        }
    }

    /// The grid's group-band label.
    const fn band(self) -> &'static str {
        match self {
            Mechanism::Dpad => "D-PAD",
            Mechanism::LeftStick => "LEFT STICK",
            Mechanism::RightStick => "RIGHT STICK",
        }
    }

    /// The half of a diagonal cell token that names the mechanism.
    const fn token(self) -> &'static str {
        match self {
            Mechanism::Dpad => "dpad",
            Mechanism::LeftStick => "ls",
            Mechanism::RightStick => "rs",
        }
    }

    /// The canonical function name for one polarity of one axis of this
    /// mechanism — what picking a direction WRITES.
    const fn function(self, vertical: bool, positive: bool) -> &'static str {
        match (self, vertical, positive) {
            (Mechanism::Dpad, true, true) => "dpad.up",
            (Mechanism::Dpad, true, false) => "dpad.down",
            (Mechanism::Dpad, false, false) => "dpad.left",
            (Mechanism::Dpad, false, true) => "dpad.right",
            (Mechanism::LeftStick, true, true) => "ly.max",
            (Mechanism::LeftStick, true, false) => "ly.min",
            (Mechanism::LeftStick, false, false) => "lx.min",
            (Mechanism::LeftStick, false, true) => "lx.max",
            (Mechanism::RightStick, true, true) => "ry.max",
            (Mechanism::RightStick, true, false) => "ry.min",
            (Mechanism::RightStick, false, false) => "rx.min",
            (Mechanism::RightStick, false, true) => "rx.max",
        }
    }
}

// ── DIAGONALS AS PRESENTATION ─────────────────────────────────────────────
// Victor: "if down and right together equals diagonal, the user does not care —
// we can present the diagonal in the piano and the user can select it, and
// behind the scenes we do down and right, so it's seamless."
//
// He is right, and it is the root cause of the evening he lost. A diagonal IS
// two simultaneous holds — that is ksx's implementation detail, not the user's
// concept. Players think in ↘ / down-forward / numpad 3, never in "two axis
// bindings held together", and no mapper in the field lets them pick one
// (Steam Input: four cardinal binding slots; reWASD's own answer: build a
// Shortcut out of two zones; MAME: four cardinals; GP2040-CE: cardinal pairs).
//
// NOTHING STORED CHANGES. A step still holds a set of ordinary bindings, so
// files stay hand-editable, the engine is untouched, and old presets keep
// working. This is the lens: `fold` reads a hold and says how to PRESENT it;
// what a pick writes is the pair, spelled exactly as `ksx map` would.
//
// MIRROR of `ksx_core::diagonal` + `ksx_core::socd::pointing`, over FUNCTION
// NAMES rather than `Binding` (ksx-studio links no ksx crate at runtime — the
// established pattern, same as the zone tables). `the_diagonal_lens_matches_
// ksx_core` pins the two against each other through the TEST-ONLY dev-deps.

/// The four diagonals — ↖ ↗ ↙ ↘, numpad 7 9 1 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Diag {
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Diag {
    const ALL: [Diag; 4] = [Diag::UpLeft, Diag::UpRight, Diag::DownLeft, Diag::DownRight];

    /// ARROW is the glyph. Screens speak arrows in this genre — SF6's input
    /// history, every Capcom move list, every arcade instruction card.
    const fn glyph(self) -> &'static str {
        match self {
            Diag::UpLeft => "↖",
            Diag::UpRight => "↗",
            Diag::DownLeft => "↙",
            Diag::DownRight => "↘",
        }
    }

    /// The numpad digit — a LOOKUP TOKEN for the tooltip and the rule line,
    /// never the label. It is how the input is written in text (Dustloop,
    /// SuperCombo) and it is already in a cab owner's `mame.ini`
    /// (`-joystick_map` uses the numpad mapping).
    const fn numpad(self) -> u8 {
        match self {
            Diag::UpLeft => 7,
            Diag::UpRight => 9,
            Diag::DownLeft => 1,
            Diag::DownRight => 3,
        }
    }

    /// The name, in words. COMPASS, not forward/back — ksx offers the mirrored
    /// spelling of every motion because player 2 is not an edge case, and
    /// "down-forward" is only true for a character facing right.
    const fn words(self) -> &'static str {
        match self {
            Diag::UpLeft => "up-left",
            Diag::UpRight => "up-right",
            Diag::DownLeft => "down-left",
            Diag::DownRight => "down-right",
        }
    }

    /// How a move list writes it (Tekken's official command lists spell `d/f`).
    /// Offered BESIDE the compass name, never instead of it.
    const fn move_list(self) -> &'static str {
        match self {
            Diag::UpLeft => "u/b",
            Diag::UpRight => "u/f",
            Diag::DownLeft => "d/b",
            Diag::DownRight => "d/f",
        }
    }

    /// `(up, right)`.
    const fn halves(self) -> (bool, bool) {
        match self {
            Diag::UpLeft => (true, false),
            Diag::UpRight => (true, true),
            Diag::DownLeft => (false, false),
            Diag::DownRight => (false, true),
        }
    }

    const fn from_halves(up: bool, right: bool) -> Diag {
        match (up, right) {
            (true, false) => Diag::UpLeft,
            (true, true) => Diag::UpRight,
            (false, false) => Diag::DownLeft,
            (false, true) => Diag::DownRight,
        }
    }

    const fn token(self) -> &'static str {
        match self {
            Diag::UpLeft => "ul",
            Diag::UpRight => "ur",
            Diag::DownLeft => "dl",
            Diag::DownRight => "dr",
        }
    }
}

/// The glyph for one cardinal polarity. ARROWS, the same family the diagonals
/// wear — a diagonal that does not look like the same family as its two parents
/// defeats the whole lens, and `◤◥◣◢` are corner *blocks*, not directions.
const fn cardinal_glyph(vertical: bool, positive: bool) -> &'static str {
    match (vertical, positive) {
        (true, true) => "↑",
        (true, false) => "↓",
        (false, false) => "←",
        (false, true) => "→",
    }
}

const fn cardinal_words(vertical: bool, positive: bool) -> &'static str {
    match (vertical, positive) {
        (true, true) => "up",
        (true, false) => "down",
        (false, false) => "left",
        (false, true) => "right",
    }
}

const fn cardinal_numpad(vertical: bool, positive: bool) -> u8 {
    match (vertical, positive) {
        (true, true) => 8,
        (true, false) => 2,
        (false, false) => 4,
        (false, true) => 6,
    }
}

/// Mirror of `ksx_core::socd::Pointing`, over a FUNCTION NAME.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pointing {
    mechanism: Mechanism,
    vertical: bool,
    /// Right for a horizontal control, UP for a vertical one.
    positive: bool,
    /// Is this the canonical extreme (`min`/`max`), or a hand-written partial
    /// deflection like `ly.-16384`?
    exact: bool,
}

/// Where this function name points, or `None` when it points nowhere.
///
/// A CENTRED AXIS IS NEVER A DIRECTION (`lx.0`) — the same rule
/// `ksx_core::socd::pointing` and `ksx_config::validate` state, which is why it
/// is never half of a diagonal either.
fn pointing(function: &str) -> Option<Pointing> {
    let lower = function.to_ascii_lowercase();
    let (base, rest) = lower.split_once('.')?;
    match base {
        "dpad" => {
            let (vertical, positive) = match rest {
                "up" => (true, true),
                "down" => (true, false),
                "left" => (false, false),
                "right" => (false, true),
                _ => return None,
            };
            Some(Pointing {
                mechanism: Mechanism::Dpad,
                vertical,
                positive,
                exact: true,
            })
        }
        "lx" | "ly" | "rx" | "ry" => {
            // `min`/`max`/`<i16>` — the same grammar `ksx_config::parse_function`
            // takes, including its `i16::MIN` fold.
            let value: i32 = match rest {
                "min" => -32767,
                "max" => 32767,
                custom => {
                    let raw: i32 = custom.parse::<i16>().ok()?.into();
                    if raw == -32768 {
                        -32767
                    } else {
                        raw
                    }
                }
            };
            if value == 0 {
                return None;
            }
            let (mechanism, vertical) = match base {
                "lx" => (Mechanism::LeftStick, false),
                "ly" => (Mechanism::LeftStick, true),
                "rx" => (Mechanism::RightStick, false),
                _ => (Mechanism::RightStick, true),
            };
            Some(Pointing {
                mechanism,
                vertical,
                positive: value > 0,
                exact: value == -32767 || value == 32767,
            })
        }
        _ => None,
    }
}

/// One PRESENTED control. Mirror of `ksx_core::diagonal::Held`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// `members` are INDICES into the original hold — the round trip is "put
    /// those strings back", so a hand-written `ly.-16384 + lx.max` displays as
    /// the diagonal and is stored byte for byte as it was written.
    Diagonal {
        diag: Diag,
        mechanisms: Vec<Mechanism>,
        members: Vec<usize>,
        exact: bool,
    },
    Plain {
        member: usize,
    },
}

/// How to PRESENT this hold.
///
/// **Per mechanism bucket, "contains both" — never exact-set-equality on the
/// whole step.** `down + forward + A` is the single most common macro step in
/// existence (the attack that ends a motion) and it folds: `A` is a passenger.
/// `down + forward + up` never folds — which diagonal would it be, and what the
/// pad publishes depends on the slot's `socd` policy, resolved at plan time,
/// which this page cannot see.
pub(crate) fn fold(hold: &[String]) -> Vec<Held> {
    let parsed: Vec<Option<Pointing>> = hold.iter().map(|f| pointing(f)).collect();
    let mut folded: Vec<(Diag, Mechanism, Vec<usize>, bool)> = Vec::new();
    let mut consumed = vec![false; hold.len()];

    for mechanism in Mechanism::ALL {
        let members: Vec<usize> = parsed
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.filter(|p| p.mechanism == mechanism).map(|_| i))
            .collect();
        if members.is_empty() {
            continue;
        }
        // POLARITIES are counted, not bindings: a hold naming `dpad.down`
        // twice is still one V−.
        let mut vertical: Option<bool> = None;
        let mut horizontal: Option<bool> = None;
        let mut split = false;
        let mut exact = true;
        for &i in &members {
            let p = parsed[i].expect("filtered above");
            exact &= p.exact;
            let slot = if p.vertical {
                &mut vertical
            } else {
                &mut horizontal
            };
            match slot {
                Some(seen) if *seen != p.positive => split = true,
                Some(_) => {}
                None => *slot = Some(p.positive),
            }
        }
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
    // control. That is the hat+stick double-binding every in-box template
    // writes — one key on `dpad.down` AND `ly.min`.
    let mut out: Vec<Held> = Vec::new();
    for diag in Diag::ALL {
        let hits: Vec<&(Diag, Mechanism, Vec<usize>, bool)> =
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

/// The cell token for a diagonal column. Contains a `:`, which no function name
/// ever does, so a diagonal pick can never be mistaken for one — the client
/// EXPANDS it to the pair before anything is stored.
fn diag_token(mechanism: Mechanism, diag: Diag) -> String {
    format!("diag:{}:{}", mechanism.token(), diag.token())
}

/// One position on the direction ring.
#[derive(Clone, Copy)]
enum RingPos {
    Cardinal { vertical: bool, positive: bool },
    Diagonal(Diag),
}

/// **↑ ↖ ← ↙ ↓ ↘ → ↗** — numpad 8 7 4 1 2 3 6 9: walk the gate from up,
/// counter-clockwise, around the bottom, back to up.
///
/// Why this order and not numpad-ascending or compass-clockwise: MOTIONS BECOME
/// SHAPES. A piano roll is read as a picture, and this is the only ordering
/// where the picture *is* the motion — a quarter-circle forward is a staircase
/// sweeping right, a half-circle is a straight 45° line, a dragon punch is
/// visibly a hook. Cardinals land on the even indices so each diagonal sits
/// literally between its two parents and the block still reads as a compass.
const RING: [RingPos; 8] = [
    RingPos::Cardinal {
        vertical: true,
        positive: true,
    },
    RingPos::Diagonal(Diag::UpLeft),
    RingPos::Cardinal {
        vertical: false,
        positive: false,
    },
    RingPos::Diagonal(Diag::DownLeft),
    RingPos::Cardinal {
        vertical: true,
        positive: false,
    },
    RingPos::Diagonal(Diag::DownRight),
    RingPos::Cardinal {
        vertical: false,
        positive: true,
    },
    RingPos::Diagonal(Diag::UpRight),
];

/// 25 zones → 37 columns: the twelve cardinal-only direction zones become three
/// rings of eight. Pinned against the tables by `the_grid_is_three_rings`.
#[cfg(test)]
const MACRO_COLUMN_COUNT: usize = 37;

/// One grid column: what it is called, what a click on it means, and which
/// band it sits under.
struct MacroColumn {
    /// The cell token — a function name, or `diag:<mech>:<diag>`.
    token: String,
    glyph: String,
    /// `maccolid`, plus `card` / `diag` for the two direction kinds.
    idcls: &'static str,
    title: String,
    band: &'static str,
}

/// Which band a control sits under. The band is not decoration — it fixes a
/// PRE-EXISTING defect: the header used to carry three identical `▲ ▼ ◀ ▶`
/// runs disambiguated only by a tooltip.
fn band_of(fn_name: &str) -> &'static str {
    match fn_name {
        "lt" | "lb" | "rb" | "rt" => "SHOULDERS",
        "A" | "B" | "X" | "Y" => "FACE",
        "guide" | "back" | "start" => "SYSTEM",
        "lthumb" => Mechanism::LeftStick.band(),
        "rthumb" => Mechanism::RightStick.band(),
        other => Mechanism::of(other).map_or("SYSTEM", Mechanism::band),
    }
}

/// The grid's columns for one persona: every non-direction control as itself,
/// and every direction MECHANISM as its eight-position ring.
///
/// 25 zones → 37 columns. The twelve cardinal-only direction zones become three
/// rings of eight, so the four diagonals are things you can point at instead of
/// things you have to know how to build.
fn macro_columns(persona: &str) -> Vec<MacroColumn> {
    let mut out: Vec<MacroColumn> = Vec::new();
    let mut rung: Vec<Mechanism> = Vec::new();
    for z in zones_for(persona).iter() {
        let Some(mechanism) = Mechanism::of(z.fn_name) else {
            out.push(MacroColumn {
                token: z.fn_name.to_owned(),
                glyph: z.label.to_owned(),
                idcls: "maccolid",
                title: format!("{} ({})", legend_label(z), z.fn_name),
                band: band_of(z.fn_name),
            });
            continue;
        };
        // The mechanism's whole ring is emitted at its FIRST direction zone;
        // the other three zones of that mechanism are already in it.
        if rung.contains(&mechanism) {
            continue;
        }
        rung.push(mechanism);
        for pos in RING {
            out.push(match pos {
                RingPos::Cardinal { vertical, positive } => {
                    let function = mechanism.function(vertical, positive);
                    MacroColumn {
                        token: function.to_owned(),
                        glyph: cardinal_glyph(vertical, positive).to_owned(),
                        idcls: "maccolid card",
                        title: format!(
                            "{}{} · {} · numpad {} · holds {function}",
                            mechanism.group(),
                            cardinal_glyph(vertical, positive),
                            cardinal_words(vertical, positive),
                            cardinal_numpad(vertical, positive),
                        ),
                        band: mechanism.band(),
                    }
                }
                RingPos::Diagonal(diag) => MacroColumn {
                    token: diag_token(mechanism, diag),
                    glyph: diag.glyph().to_owned(),
                    idcls: "maccolid diag",
                    // `move_list` is FACING-RELATIVE and is labelled as such.
                    // ksx has no notion of facing — it publishes a direction,
                    // not a side of the screen — and it already ships the
                    // mirrored spelling of every motion because player 2 is
                    // not an edge case. Printing a bare "d/f" beside a compass
                    // name reads as a second name for the same fact; it is a
                    // second name for the fact HALF THE TIME, and is d/b for
                    // the player standing on the right.
                    title: format!(
                        "{}{} · {} · numpad {} · {} in a move list, facing right · one pick, and \
                         ksx writes {} + {}",
                        mechanism.group(),
                        diag.glyph(),
                        diag.words(),
                        diag.numpad(),
                        diag.move_list(),
                        mechanism.function(true, diag.halves().0),
                        mechanism.function(false, diag.halves().1),
                    ),
                    band: mechanism.band(),
                },
            });
        }
    }
    out
}

/// Every mechanism THIS SLOT's own bound direction keys drive. An inert `None`
/// row does not count: a placeholder is a function the preset lists, not a
/// direction the player can produce (same rule as `driven_mechanisms` there).
fn driven_mechanisms(slot: Option<&MapperSlot>) -> Vec<Mechanism> {
    let mut out: Vec<Mechanism> = Vec::new();
    let Some(slot) = slot else {
        return out;
    };
    for (function, keys) in &slot.bindings {
        if keys
            .iter()
            .all(|k| k.is_empty() || k.eq_ignore_ascii_case("None"))
        {
            continue;
        }
        if let Some(m) = Mechanism::of(function) {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    out
}

/// The sentence above the motion buttons: which mechanism they will write, and
/// why that is the one.
fn macro_motion_line(slot: Option<&MapperSlot>) -> String {
    let driven = driven_mechanisms(slot);
    let pick = driven.first().copied().unwrap_or(Mechanism::Dpad);
    let tail = "Each one appends its steps to the macro below — the MIDDLE step of a \
                quarter-circle is the diagonal, and a 360 is four of them. You can tick any of \
                them yourself in that group's ↖ ↗ ↙ ↘ columns.";
    match driven.len() {
        0 => format!(
            "These write {} — this preset binds no direction keys of its own, so there is \
             nothing to match. If the game reads a stick, retick the rows. {tail}",
            pick.describe()
        ),
        1 => format!(
            "These write {} — the same mechanism this preset's own direction keys drive, so \
             the game reads them. (A motion written on the other mechanism is published \
             faithfully and read by nobody: that is the trap.) {tail}",
            pick.describe()
        ),
        _ => format!(
            "These write {}. This preset's own direction keys drive {}, so either would be \
             read — a pad has three ways to say \"right\" and a game reads whichever one it \
             was written for. {tail}",
            pick.describe(),
            driven
                .iter()
                .map(|m| m.describe())
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    }
}

/// The slot-wide `macros = "off"` switch, in words — and the exact line to
/// change. Empty for every slot that runs macros, which is every slot until
/// somebody says otherwise.
///
/// A SENTENCE and not a button, deliberately. The switch lives in config.toml
/// (or in the games.toml profile), and Studio writes presets only — every verb
/// on this page goes through `map`/`map-macro`. A toggle that silently did
/// nothing is worse than a line that says which file to edit, so this is the
/// line. Mirrored in MapIsland.ts `slotMacrosLineFor`.
fn slot_macros_line(slot: Option<&MapperSlot>) -> String {
    match slot {
        Some(slot) if slot.macros_off => format!(
            "Slot {} says macros = \"off\" — the TOURNAMENT SWITCH. Nothing in this card runs on \
             it, whatever each macro's own switch says, and nothing is deleted. To bring them \
             back, set macros = \"on\" on that [[slot]] in config.toml (or on the slot of the \
             games.toml profile you are running) and reload the session.",
            slot.number
        ),
        _ => String::new(),
    }
}

/// The one-line summary above the grid.
fn macro_head(
    payload: &MapPayload,
    selected: Option<&MapperSlot>,
    mac: Option<&MacroView>,
) -> String {
    match mac {
        Some(mac) => format!(
            "{} — {} step{} · {} ms total{}",
            mac.name,
            mac.steps.len(),
            if mac.steps.len() == 1 { "" } else { "s" },
            total_ms(mac),
            // Loud, and in the head line, because everything under it
            // describes something that will not happen.
            if mac.disabled {
                " · DISABLED (keeps its steps and its trigger; never runs)"
            } else {
                ""
            }
        ),
        None if payload.macros.available => {
            format!("\"{}\" has no macros yet", preset_name(payload, selected))
        }
        None => "no macro loaded yet".to_owned(),
    }
}

/// The preset every macro line is about — the snapshot's own name when the
/// provider gave one, else the selected slot's.
fn preset_name(payload: &MapPayload, selected: Option<&MapperSlot>) -> String {
    if payload.macros.preset.is_empty() {
        selected
            .map_or("<PRESET>", |s| s.preset.as_str())
            .to_owned()
    } else {
        payload.macros.preset.clone()
    }
}

/// The three policies, as the file holds them right now — the READABLE half of
/// the three selects beside them (those are draft controls and are hidden
/// without JavaScript, this line never is).
fn macro_policy_line(mac: Option<&MacroView>) -> String {
    match mac {
        Some(mac) => {
            let repeat = if mac.repeat.is_empty() {
                "once"
            } else {
                mac.repeat.as_str()
            };
            let rate = match (mac.turbo_hz, mac.gap_ms) {
                (Some(hz), _) => format!(" ({hz} Hz)"),
                (None, Some(ms)) => format!(" ({ms} ms gap)"),
                (None, None) => String::new(),
            };
            format!(
                "on release: {} · retrigger: {} · interrupt: {} · repeat: {repeat}{rate}",
                mac.on_release, mac.retrigger, mac.interrupt
            )
        }
        None => String::new(),
    }
}

/// The macro's REPEAT arithmetic, in words — the same live-math treatment the
/// duration field got, for the same reason: `turbo_hz = 30` on a 50 ms
/// sequence is not 30 Hz and never could be, and the only honest thing to do
/// with that number is say so while it is being typed.
///
/// Mirrored in MapIsland.ts `turboMath`, and the arithmetic is the one
/// `ksx_core::Macro::turbo_gap_ms` runs, so the card and the engine cannot
/// drift.
fn turbo_math(mac: Option<&MacroView>) -> String {
    let Some(mac) = mac else {
        return String::new();
    };
    match mac.repeat.as_str() {
        "while-held" => "Holding the trigger starts the sequence again the instant it ends, with \
             NO gap between runs — the right shape for a MOTION whose last step flows into its \
             first, and the wrong one for auto-fire (a game reads two touching runs as one long \
             hold)."
            .to_owned(),
        "turbo" => {
            let run = macro_total_ms(mac);
            let (gap, why) = turbo_gap_ms(mac, run);
            let cycle = run + gap;
            if cycle == 0 {
                return "This macro has no steps, so there is nothing to repeat.".to_owned();
            }
            let effective = (1_000 + cycle / 2) / cycle;
            let asked = match (mac.turbo_hz, mac.gap_ms) {
                (Some(hz), _) => format!("Requested {hz} Hz"),
                (None, Some(ms)) => format!("Requested a {ms} ms gap"),
                (None, None) => {
                    "No rate given — a turbo with no rate is refused by the loader".to_owned()
                }
            };
            format!(
                "{asked} → effective ~{effective} Hz, because the sequence itself is {run} ms \
                 long and the neutral gap between runs is {gap} ms{why}: one full press/release \
                 cycle takes {cycle} ms. Each half has to survive a 60 Hz poll ({MIN_STEP_MS} \
                 ms), which is what caps this — the rate is capped, never refused."
            )
        }
        _ => "One run per press. Holding the trigger changes nothing, which is what stops a \
             special move turning into a machine gun when a panel switch bounces."
            .to_owned(),
    }
}

/// The macro's whole run at the durations the engine will really use.
fn macro_total_ms(mac: &MacroView) -> u32 {
    mac.steps.iter().map(effective_ms).sum()
}

/// The neutral window between two turbo runs, and WHY it is that number.
/// Mirrors `ksx_core::Macro::turbo_gap_ms`.
fn turbo_gap_ms(mac: &MacroView, run: u32) -> (u32, &'static str) {
    let asked = match (mac.turbo_hz, mac.gap_ms) {
        (Some(hz), _) => {
            let hz = hz.clamp(1, TURBO_MAX_HZ);
            let cycle = (1_000 + hz / 2) / hz;
            cycle.saturating_sub(run)
        }
        (None, Some(ms)) => ms,
        (None, None) => MIN_STEP_MS,
    };
    if asked < MIN_STEP_MS {
        (
            MIN_STEP_MS,
            " (raised to the sampling floor — a gap the game never samples is not a gap, it \
             reads as one long hold)",
        )
    } else {
        (asked, "")
    }
}

/// What this card can do right now, in the user's words rather than the
/// architecture's. Never empty, and it tells the three states apart: the
/// provider could not read macros / the preset has none / here is one, and
/// here is what Save does. (Since v12 the "read-only draft" state is gone —
/// there is a Save button, and the note says what it writes.)
fn macro_note(payload: &MapPayload, mac: Option<&MacroView>) -> String {
    if !payload.macros.available {
        return format!(
            "This preset's macros could not be read ({}), so there is nothing to edit and \
             nothing here can be saved. That is NOT the same as \"this preset has no macros\" \
             — it means nobody could tell this page either way.",
            payload.macros.reason
        );
    }
    if payload.macros.macros.is_empty() {
        return "This preset has no macros yet. Type a name above and press ＋ New macro: it \
                is written into the preset straight away (one empty 50 ms step), and then you \
                paint the grid and press Save macro."
            .to_owned();
    }
    let Some(mac) = mac else {
        return "Pick a macro above to edit it, or type a name and press ＋ New macro.".to_owned();
    };
    format!(
        "Steps and policies are a DRAFT until you press Save macro — that writes the whole \
         \"{}\" table into the preset file (a timestamped backup is taken first) and swaps it \
         into a running session with the pads left plugged. New, Rename and Delete write \
         immediately. Every one of them can be undone from the toast it leaves.",
        mac.name
    )
}

fn scalar_slots(
    payload: &MapPayload,
    selected: Option<&MapperSlot>,
    mac: Option<&MacroView>,
    flash: Option<&str>,
) -> serde_json::Value {
    let slot_line = match selected {
        Some(s) => format!("P{} · {} · {}", s.number, s.persona_label, s.preset),
        None => "no mappable slots".to_owned(),
    };
    serde_json::json!({
        "slotLine": slot_line,
        "sourceLine": format!(
            "{} — config root: {}",
            payload.mapper.source, payload.mapper.config_root
        ),
        "reasonLine": reason_line(payload),
        "cliLine": cli_line(selected),
        // FIX 1: the copyable remedy, carrying this machine's profile flag.
        "daemonCmd": daemon_command(&payload.session),
        "backupLine": backup_line(selected),
        // v14: the preset surface's identity block. Derived, never a new
        // payload field — the slot already carries the preset name and the
        // snapshot the config root.
        "presetLine": selected.map_or("(no preset)", |s| s.preset.as_str()),
        "presetPath": match selected {
            Some(s) => format!(
                r"{}\presets\{}.toml",
                payload.mapper.config_root, s.preset
            ),
            None => payload.mapper.config_root.clone(),
        },
        "backupFact": match selected.and_then(|s| s.backup.as_deref()) {
            Some(label) => format!("newest {label}"),
            None => "none yet — the first restore writes one".to_owned(),
        },
        "modalPrompt": "",
        "modalBinding": "",
        "countdownText": "",
        "barStyle": "width:100%",
        "conflictLine": "",
        // v9: the no-JS action report. A form POST 303s back here with the
        // outcome in `?flash=`, and this line is where the page says it — the
        // only feedback channel a page without JavaScript has.
        "savedLine": flash.unwrap_or(""),
        // Which slot every no-JS form outside the legend list posts about.
        "slotNum": selected.map(|s| s.number).unwrap_or(payload.selected).to_string(),
        // Auto-save is invisible until it says so (Victor: "where is save?").
        // Empty on SSR — the page has not written anything yet.
        "savedAt": "",
        "generatedAt": payload.mapper.generated_at,
        // FEATURE 2's multi-select is a JS enhancement: SSR always paints the
        // toggle OFF and no selection. (`.seltoggle` is hidden until map.ts
        // marks the island `.js`, so a no-JS page keeps exactly the v6
        // single-click-to-learn behaviour instead of growing a dead button.)
        "selToggleCls": SEL_TOGGLE_OFF,
        "selToggleLabel": SEL_TOGGLE_LABEL_OFF,
        "selCountLine": "",
        // The preset-actions card: a class string, never a show (ledger #13
        // — its bindings must survive; the off look is just a class).
        "actionsCls": if payload.session.reachable {
            "card pactions"
        } else {
            "card pactions off"
        },
        // ── v11: the macro editor ──────────────────────────────────────
        // Not one new `createShow` between them: every state here is a class
        // string on an element that is always in the DOM (ledger #13/#14 —
        // shows are POSITIONAL, so a new one in the middle of the document
        // silently shifts every panel after it). The card dims when there is
        // no macro data rather than vanishing, exactly like the preset card.
        "macroHead": macro_head(payload, selected, mac),
        "macroRuleLine": MACRO_RULE_LINE,
        "macroRingLine": MACRO_RING_LINE,
        "macroPolicyLine": macro_policy_line(mac),
        "macroNote": macro_note(payload, mac),
        "macroTriggerLine": macro_trigger_line(mac),
        // Empty when the preset holds no macro: `data-fn=""` is inert in the
        // click delegation, which is the honest state for a trigger that has
        // nothing to start. Never a placeholder name the daemon would reject.
        "macroFnName": mac.map_or_else(String::new, |m| macro_function(&m.name)),
        "macroName": mac.map_or_else(String::new, |m| m.name.clone()),
        "macroCliLine": macro_cli(&preset_name(payload, selected), mac),
        "macroToml": mac.map_or_else(String::new, macro_toml),
        "macroCardCls": if payload.macros.available {
            "card macrocard"
        } else {
            "card macrocard off"
        },
        "macroGridCls": if mac.is_none_or(|m| m.steps.is_empty()) {
            "macgrid empty"
        } else {
            "macgrid"
        },
        // v12: the trigger block is inert while there is no macro to start.
        "macroTrigCls": if mac.is_some() {
            "mactrigger"
        } else {
            "mactrigger off"
        },
        // Client-only: an SSR paint has edited nothing, has no step selected
        // for the duration editor, and cannot save (every write on this card
        // is a fetch). "saved" is the honest resting state for a card whose
        // grid is exactly what the file says.
        "macroDirtyLine": if mac.is_some() { "saved" } else { "" },
        "macroSaveCls": "btn btn-mini macsave off",
        // v14: the per-macro ON/OFF switch. Two class-string scalars and no
        // show, like everything else on this card, and the button reads as the
        // STATE it is in rather than the action it performs — "Disable" on a
        // macro that is already off is the one label a person in a hurry
        // cannot read correctly.
        "macroEnableCls": match mac {
            Some(m) if m.disabled => "btn btn-mini macen offstate",
            Some(_) => "btn btn-mini macen on",
            // Nothing loaded: inert, not a switch for a macro that is not there.
            None => "btn btn-mini macen off dead",
        },
        "macroEnableLabel": match mac {
            Some(m) if m.disabled => "DISABLED — click to enable",
            _ => "Enabled",
        },
        // v14: the SLOT's master switch, in words and never as a button — it
        // lives in config.toml, and Studio has no config writer. See
        // `slot_macros_line`.
        "slotMacrosLine": slot_macros_line(selected),
        // FIX 2: this line REPORTS which step the frame maths is about; it no
        // longer instructs anybody to select one, because a duration is now
        // edited in the row's own box. `macroDurValue`/`macroDurCls` went with
        // the panel field they described — the number and the warn class are
        // per-ROW now (`macro_rows`), which is where the number is.
        "macroStepLine": "every step's time is its own box on its own row — type in the row you \
                          want",
        // v15/FIX 2: Save's inline question about short steps. A class string
        // plus its sentence and no show, like everything else here. An SSR
        // paint has asked nothing — every write on this card is a fetch.
        "macroConfirmCls": "macconfirm off",
        "macroConfirmLine": "",
        // v15/FIX 1c: which mechanism the "common motions" buttons write. The
        // buttons are client-only (they edit a draft), but WHICH MECHANISM
        // THIS PRESET DRIVES is a fact about the file, so it is server-side.
        "macroMotionLine": macro_motion_line(selected),
        // The frame maths, at the selector's default rate — no step is
        // selected on an SSR paint, so this is the floor sentence.
        "macroMathLine": frame_math(None, SSR_RATE_HZ),
        // v13: the repeat policy and its rate, with the SAME live-math
        // treatment the duration field got — a rate the sampler cannot deliver
        // says BOTH numbers, on screen, while it is being typed.
        "macroTurboLine": turbo_math(mac),
        "macroTurboValue": mac.map_or_else(String::new, |m| match (m.turbo_hz, m.gap_ms) {
            (Some(hz), _) => hz.to_string(),
            (None, Some(ms)) => ms.to_string(),
            (None, None) => String::new(),
        }),
    })
}

fn show_values(
    payload: &MapPayload,
    selected: Option<&MapperSlot>,
    flash: Option<&str>,
) -> [(&'static str, bool); MAP_SHOW_COUNT] {
    let art = selected.map(|s| art_for(&s.persona));
    let live = learnable(payload) && selected.is_some();
    let running = payload.session.reachable && payload.session.running;
    // Same rule as the status page's flash: an outcome that starts with
    // "error" is reported as one, everything else as a plain confirmation.
    let flash = flash.map(str::trim).filter(|f| !f.is_empty());
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    [
        ("show:pillRunning", running),
        (
            "show:pillIdle",
            payload.session.reachable && !payload.session.running,
        ),
        ("show:pillDown", !payload.session.reachable),
        // FIX 0: client-only — set by the page's own Pause action, so the
        // user cannot forget the cabinet is paused because they opened the
        // mapper. Never SSR-true.
        ("show:pillPaused", false),
        // FIX 1: the unmissable banner, first child of <main>.
        ("show:noDaemon", !payload.session.reachable),
        // FIX 0: running-session banner + the one-click "Pause emulation & map".
        ("show:sessionRunning", running),
        // FIX 0: the road back — client-only, same flag as the pill.
        ("show:pausedBar", false),
        ("show:readOnly", !live),
        ("show:canLearn", live),
        ("show:artXbox", art == Some(crate::render::ART_XBOX)),
        ("show:artDs4", art == Some(crate::render::ART_DS4)),
        // FIX 2: the third restore destination only exists when a backup does.
        (
            "show:hasBackup",
            selected.is_some_and(|s| s.backup.is_some()),
        ),
        // v9: the no-JS flash, server-rendered from ?flash=. The client keeps
        // reporting through the toast stack instead — map.ts blanks these on
        // adoption, so nothing is said twice.
        ("show:savedOk", flash.is_some() && !flash_err),
        ("show:savedErr", flash_err),
        // The learn modal and its three inner states: client-only, never
        // SSR-open.
        ("show:modalOpen", false),
        ("show:modalListening", false),
        ("show:modalBound", false),
        ("show:modalConflict", false),
        // v7 multi-select: client-only, nothing is selected on an SSR paint.
        ("show:selBar", false),
    ]
}

/// Slot ids of every slot named `name`, slot-table order (== document order).
fn named_slot_ids(module: &IrModule, name: &str) -> Vec<u16> {
    module
        .slots
        .entries()
        .iter()
        .filter(|e| module.strings.get(e.name_str_idx).is_ok_and(|n| n == name))
        .map(|e| e.slot_id)
        .collect()
}

fn build_slots(module: &IrModule, payload: &MapPayload, flash: Option<&str>) -> SlotData {
    let selected = selected_slot(payload);
    let mac = selected_macro(payload);
    let mac = mac.as_ref();
    let scalars = scalar_slots(payload, selected, mac, flash).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    let tabs = slot_tabs(payload, selected);
    let live = learnable(payload) && selected.is_some();
    let write = writable(payload) && selected.is_some();
    let zones = selected
        .map(|slot| zone_rows(slot, live))
        .unwrap_or(SlotValue::array(Vec::new()));
    let legend = selected
        .map(|slot| legend_rows(slot, live, write))
        .unwrap_or(SlotValue::array(Vec::new()));
    for (name, value) in [
        (LIST_SLOT_TABS, tabs.clone()),
        (LIST_SLOT_ZONES, zones.clone()),
        (LIST_SLOT_ZONES_2, zones),
        (LIST_SLOT_LEGEND, legend),
        // Explicitly empty, so the toast stack can never SSR a stale report.
        (LIST_SLOT_TABS_2, tabs),
        (LIST_SLOT_TOASTS, SlotValue::array(Vec::new())),
        // v11's piano roll. `None` for the selected step: an SSR paint has
        // pointed the duration editor at nothing.
        (LIST_SLOT_MACRO_TABS, macro_tabs(payload, mac)),
        (LIST_SLOT_MACRO_GROUPS, macro_groups(selected)),
        (LIST_SLOT_MACRO_COLS, macro_cols(selected)),
        (LIST_SLOT_MACRO_ROWS, macro_rows(mac, selected, None)),
        (LIST_SLOT_MACRO_CELLS, macro_cells(mac, selected, None)),
    ] {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    // Shows BY NAME since compiler 0.3.1 (ledger #4 closed) — a renamed
    // condition signal degrades to its authored `false` default instead of
    // silently inheriting the next show's boolean.
    for (name, value) in show_values(payload, selected, flash) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, SlotValue::Bool(value));
        }
    }
    slots
}

/// Same anti-flash CSS as the status page (render.rs PERSONALITY_CSS).
/// Kept byte-identical to it; `tests/contrast.rs` pins both against the
/// `--bg`/`--text` tokens in studio.css so the copies cannot drift apart.
const PERSONALITY_CSS: &str = "body{background:#120c1c;color:#f0ebe0;margin:0}\
@media (prefers-color-scheme:light){body{background:#f6f3ee;color:#1c1428}}";

/// Render `/map` for one payload. The `selected` inside the payload drives
/// the SSR slot pick; the client keeps its own selection after hydration.
///
/// `flash` is the outcome of the no-JS form POST that redirected here (v9) —
/// the same post-redirect-get shape the status page has always used.
pub(crate) fn render_map(
    page: &EmbeddedPage,
    payload: &MapPayload,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, payload, flash);
    let prefix = body_prefix(payload, "/map");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — mapper",
        route_pattern: "/map",
        manifest: &page.manifest,
        config_script: None,
        // The SAFE server-data path (serde_json-escaped, surfaced as
        // window.__FORMA_CONFIG__), as opposed to config_script, which is a raw
        // trusted escape hatch. ksx injects its island props through the FMIR
        // slot table instead, so neither is used — but the distinction matters
        // the day anything server-derived does need to reach the client.
        config_json: None,
        body_class: None,
        personality_css: Some(PERSONALITY_CSS),
        body_prefix: Some(&prefix),
        render_mode: RenderMode::Phase2SsrReconcile,
        ir_module: Some(&page.module),
        slots: Some(&slots),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{LearnView, SessionView};
    use crate::snapshot::{MacroSnapshot, MapperSnapshot};
    use std::collections::BTreeMap;

    fn step(
        hold: &[&str],
        ms: Option<u32>,
        frames: Option<u32>,
        allow_short: bool,
    ) -> MacroStepView {
        MacroStepView {
            hold: hold.iter().map(|f| (*f).to_owned()).collect(),
            ms,
            frames,
            allow_short,
        }
    }

    /// The documented hadouken (docs/INPUT-TRANSFORMS.md §1c), as the file
    /// spells it.
    fn hadouken() -> MacroView {
        MacroView {
            name: "hadouken".to_owned(),
            steps: vec![
                step(&["dpad.down"], Some(50), None, false),
                step(&["dpad.down", "dpad.right"], Some(50), None, false),
                step(&["dpad.right"], None, Some(3), false),
                step(&["A"], Some(50), None, false),
            ],
            on_release: "finish".to_owned(),
            retrigger: "ignore".to_owned(),
            interrupt: "none".to_owned(),
            repeat: "once".to_owned(),
            turbo_hz: None,
            gap_ms: None,
            triggers: vec!["P".to_owned()],
            disabled: false,
        }
    }

    fn slot(number: u8, persona: &str, preset: &str) -> MapperSlot {
        let mut bindings = BTreeMap::new();
        bindings.insert("A".to_owned(), vec!["G".to_owned()]);
        bindings.insert("B".to_owned(), vec!["F".to_owned()]);
        bindings.insert("lx.min".to_owned(), vec!["M".to_owned()]);
        bindings.insert("start".to_owned(), Vec::new()); // cleared → unbound
                                                         // One control that AUTO-FIRES, so the legend badge and its title ride
                                                         // the ordinary render tests rather than only their own.
        let turbo = BTreeMap::from([("B".to_owned(), 12)]);
        MapperSlot {
            number,
            persona: persona.to_owned(),
            persona_label: if persona == "playstation" {
                "PlayStation (DS4)".to_owned()
            } else {
                "Xbox 360".to_owned()
            },
            preset: preset.to_owned(),
            keyboard: r"HID\VID_D209&PID_0430&REV_0056&MI_00".to_owned(),
            bindings,
            backup: Some("2026-08-05 14:32:07 UTC".to_owned()),
            turbo,
            macros_off: false,
        }
    }

    pub(super) fn sample() -> MapPayload {
        MapPayload {
            mapper: MapperSnapshot {
                generated_at: "2026-08-05 12:00:00 UTC".into(),
                source: "slots of profile \"Steam\" (games.toml)".into(),
                profile: Some("Steam".into()),
                config_root: r"C:\Users\arcade\AppData\Roaming\ksx".into(),
                slots: vec![
                    slot(1, "xbox360", "IPAC P1"),
                    slot(2, "xbox360", "IPAC P2"),
                    slot(3, "playstation", "IPAC P3"),
                ],
            },
            session: SessionView {
                reachable: true,
                running: false,
                line: "idle".into(),
                profile: None,
            },
            learn: LearnView {
                ok: true,
                state: "idle".into(),
                ..LearnView::default()
            },
            selected: 1,
            macros: MacroSnapshot::read("IPAC P1", vec![hadouken()]),
            macro_selected: String::new(),
        }
    }

    fn page() -> EmbeddedPage {
        EmbeddedPage::load("/map").expect("embedded mapper page must load")
    }

    /// The page with its two MACHINE-readable blocks removed: forma-ir's
    /// native `data-forma-props` attribute (ledger #8, adopted 2026-08-06 —
    /// it repeats every injected slot value, and its KEYS are slot names like
    /// `conflictLine`) and the `__ksx-payload` data block. Use it for
    /// assertions about what the page SAYS — a negative one especially, since
    /// a word that must not appear in the prose now appears in both blocks as
    /// a matter of course.
    fn visible(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(i) = rest.find("data-forma-props=\"") {
            out.push_str(&rest[..i]);
            let after = &rest[i + "data-forma-props=\"".len()..];
            rest = after.split_once('"').map_or("", |(_, r)| r);
        }
        out.push_str(rest);
        let mut cleaned = String::with_capacity(out.len());
        let mut rest = out.as_str();
        let open = "<script id=\"__ksx-payload\" type=\"application/json\">";
        while let Some(i) = rest.find(open) {
            cleaned.push_str(&rest[..i]);
            let after = &rest[i + open.len()..];
            rest = after.split_once("</script>").map_or("", |(_, r)| r);
        }
        cleaned.push_str(rest);
        // And the walker's OWN props block. forma-ir 0.2.0 spills props over
        // `INLINE_PROPS_MAX_BYTES` out of the inline attribute stripped above
        // and into this shared `<script type="application/json">` — same data,
        // new location. /map's props are the largest on the site, so they all
        // moved, and every one of their slot values would otherwise read as
        // page text: "conflictLine" made `!visible.contains("conflict")` fail,
        // and an empty title slot made the `title=""` check fail. Neither is
        // anything a user can see.
        let mut out = String::with_capacity(cleaned.len());
        let mut rest = cleaned.as_str();
        let open = "<script id=\"__forma_islands\" type=\"application/json\">";
        while let Some(i) = rest.find(open) {
            out.push_str(&rest[..i]);
            let after = &rest[i + open.len()..];
            rest = after.split_once("</script>").map_or("", |(_, r)| r);
        }
        out.push_str(rest);
        out
    }

    /// The injected text inside the FIRST element carrying `class="<cls>"`.
    /// SSR wraps every slot value in `<!--f:tN-->…<!--/f:tN-->` markers, so
    /// "this class holds that glyph" cannot be asserted as one substring.
    fn text_in(html: &str, cls: &str) -> Option<String> {
        let start = html.find(&format!("class=\"{cls}\">"))?;
        let rest = &html[start..];
        let open = rest.find("-->")? + 3;
        let end = rest[open..].find("<!--")? + open;
        Some(rest[open..end].to_owned())
    }

    /// Both tables cover exactly the 25 mappable functions, once each — a
    /// zone the preset vocabulary cannot store, or a function without a
    /// zone, is a build error here, not a dead click in the browser.
    #[test]
    fn zone_tables_cover_every_mappable_function() {
        const FUNCTIONS: [&str; 25] = [
            "A",
            "B",
            "X",
            "Y",
            "lb",
            "rb",
            "lt",
            "rt",
            "back",
            "start",
            "guide",
            "lthumb",
            "rthumb",
            "dpad.up",
            "dpad.down",
            "dpad.left",
            "dpad.right",
            "lx.min",
            "lx.max",
            "ly.min",
            "ly.max",
            "rx.min",
            "rx.max",
            "ry.min",
            "ry.max",
        ];
        for table in [&ZONE_XBOX, &ZONE_DS4] {
            let mut names: Vec<&str> = table.iter().map(|z| z.fn_name).collect();
            names.sort_unstable();
            let mut want = FUNCTIONS.to_vec();
            want.sort_unstable();
            assert_eq!(names, want);
            // Every zone stays inside the stage.
            for z in table.iter() {
                assert!(
                    z.cx - z.w / 2.0 >= 0.0 && z.cx + z.w / 2.0 <= 100.0,
                    "{}",
                    z.fn_name
                );
                assert!(
                    z.cy - z.h / 2.0 >= 0.0 && z.cy + z.h / 2.0 <= 100.0,
                    "{}",
                    z.fn_name
                );
            }
            // Zones are pure hit areas: two rects that overlap are two
            // controls fighting for the same click. Pairwise disjoint, no
            // exceptions — the stick wedges RING the L3/R3 hub, the face
            // diamond and dpad cross keep their diagonals apart.
            for (i, a) in table.iter().enumerate() {
                for b in table.iter().skip(i + 1) {
                    let ox = (a.cx + a.w / 2.0).min(b.cx + b.w / 2.0)
                        - (a.cx - a.w / 2.0).max(b.cx - b.w / 2.0);
                    let oy = (a.cy + a.h / 2.0).min(b.cy + b.h / 2.0)
                        - (a.cy - a.h / 2.0).max(b.cy - b.h / 2.0);
                    assert!(
                        ox <= 0.0 || oy <= 0.0,
                        "zones {} and {} overlap by {ox:.2}% × {oy:.2}%",
                        a.fn_name,
                        b.fn_name
                    );
                }
            }
        }
    }

    /// Pins the mapper IR's slot layout: scalars by name, the two list slot
    /// names, the show count, and the single MapIsland island.
    #[test]
    fn embedded_map_ir_slot_layout_matches_the_seam() {
        let page = page();
        let module = page.module();
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&sample(), None, Some(&hadouken()), None);
        for key in scalars.as_object().unwrap().keys() {
            assert!(
                names.contains(&key.as_str()),
                "scalar slot '{key}' missing from the mapper IR; slots: {names:?}"
            );
        }
        let array_slots: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("list:") && n.ends_with(":array"))
            .collect();
        assert_eq!(
            array_slots,
            [
                LIST_SLOT_TABS,
                LIST_SLOT_ZONES,
                LIST_SLOT_ZONES_2,
                LIST_SLOT_LEGEND,
                // Document order: the row bar precedes the scroller that
                // holds the column headers and the matrix.
                LIST_SLOT_MACRO_TABS,
                LIST_SLOT_MACRO_ROWS,
                // v16: the group band is the first child of `.macscroll`,
                // above the glyph row it labels.
                LIST_SLOT_MACRO_GROUPS,
                LIST_SLOT_MACRO_COLS,
                LIST_SLOT_MACRO_CELLS,
                LIST_SLOT_TABS_2,
                LIST_SLOT_TOASTS
            ],
            "mapper list slot names drifted; slots: {names:?}"
        );
        // Shows, BY NAME and as a SET (ledger #4, adopted 2026-08-06): what
        // used to be a count assertion — the only guard a positional seam
        // could have — now names every slot on both sides.
        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> = show_values(&sample(), None, None)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "mapper show slots drifted between MapIsland.ts and show_values()"
        );
        assert_eq!(
            ir_shows.len(),
            MAP_SHOW_COUNT,
            "MAP_SHOW_COUNT is stale; slots: {names:?}"
        );
        // Ledger #8, adopted 2026-08-06: `slot_ids` are populated, so
        // forma-ir emits `data-forma-props` and the `__forma_islands` block
        // this seam used to hand-build is gone. This assertion used to be
        // `slot_ids.is_empty()` — a tripwire written to fail here.
        let islands = module.islands.entries();
        assert_eq!(islands.len(), 1, "expected exactly one island");
        assert_eq!(
            module.strings.get(islands[0].name_str_idx).unwrap(),
            ISLAND_COMPONENT
        );
        assert!(
            !islands[0].slot_ids.is_empty(),
            "island slot_ids are empty — compiler regressed to 0.2.0 \
             behaviour and native data-forma-props will not be emitted"
        );
        // The slot contract: injected == rendered, both ways, plus the
        // anonymous set. `assert_island_slot_contract` carries the reason a
        // name-EXISTS check is not enough (a twin declaration renames the
        // rendered binding to `#2` and injection then fills a dead slot).
        let injected: Vec<&str> = scalars
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .chain([
                LIST_SLOT_TABS,
                LIST_SLOT_ZONES,
                LIST_SLOT_ZONES_2,
                LIST_SLOT_LEGEND,
                LIST_SLOT_TABS_2,
                LIST_SLOT_TOASTS,
                LIST_SLOT_MACRO_TABS,
                LIST_SLOT_MACRO_GROUPS,
                LIST_SLOT_MACRO_COLS,
                LIST_SLOT_MACRO_ROWS,
                LIST_SLOT_MACRO_CELLS,
            ])
            .chain(seam_shows.iter().copied())
            .collect();
        crate::render::assert_island_slot_contract(
            module,
            &injected,
            &MAP_CLIENT_ONLY_SLOTS,
            &MAP_ANONYMOUS_SLOTS,
        );
    }

    /// The mapper wears the same face as the status page: the icon links go
    /// through the shared oracle in `render.rs`, so the two pages cannot
    /// declare different icons.
    #[test]
    fn the_mapper_head_is_complete() {
        let out = render_map(&page(), &sample(), None);
        crate::render::assert_complete_head("/map", &out.html);
    }

    /// The big one: real bindings from the payload land as key tags in the
    /// SSR LEGEND and as the small on-zone TAGS (v6 — both readers), the
    /// zones are positioned by the table, the right art is referenced, and
    /// the slot strip shows the context.
    #[test]
    fn render_puts_bindings_in_the_legend_and_on_the_zones() {
        let out = render_map(&page(), &sample(), None);
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        // Slot context strip.
        assert!(out.html.contains("P1 · Xbox 360 · IPAC P1"), "{}", out.html);
        assert!(out.html.contains("P2 · IPAC P2"), "{}", out.html);
        // The Xbox art for an xbox360 slot, exactly once on the stage.
        assert!(
            out.html.contains(r#"src="/_assets/pad-xbox.svg""#),
            "{}",
            out.html
        );
        // Zones: data-fn + table-derived position + the fn—key tooltip +
        // the small binding tag — but never the old crammed label+key chip
        // (zlabel/zkey must not come back; unbound tags render EMPTY, not
        // "—").
        assert!(out.html.contains(r#"data-fn="A""#), "{}", out.html);
        assert!(out.html.contains(r#"data-fn="dpad.up""#), "{}", out.html);
        assert!(out.html.contains(r#"title="A — G""#), "{}", out.html);
        assert!(out.html.contains(r#"class="ztag""#), "{}", out.html);
        assert!(
            !out.html.contains(r#"class="ztag">—<"#),
            "unbound zones must render an empty tag, not a dash: {}",
            out.html
        );
        assert!(!out.html.contains("zlabel"), "zone inline label came back");
        assert!(!out.html.contains("zkey"), "zone inline key tag came back");
        // v7: the multi-select toggle ships in the controller card's header
        // (CSS keeps it hidden until map.ts marks the island `.js`), and
        // nothing is selected on an SSR paint.
        assert!(
            out.html.contains(r#"data-act="multi-toggle""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("Select multiple"), "{}", out.html);
        assert!(
            !out.html.contains("controls selected"),
            "the selection bar must never SSR: {}",
            out.html
        );
        // The preset-actions card: save semantics + both safety nets, live
        // (the daemon is reachable in the sample).
        assert!(
            out.html.contains(r#"class="card pactions""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("Every binding saves immediately"),
            "{}",
            out.html
        );
        assert!(out.html.contains("Undo this session"), "{}", out.html);
        // FIX 2: the label names the LAYOUT it writes. The abstract word
        // "defaults" must not survive anywhere on the page — it is what made
        // Victor read a desktop-keyboard reset as "put my panel map back".
        assert!(
            out.html
                .contains("Reset to generic keyboard layout (S/D/A/W…)"),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains("Restore built-in defaults"),
            "the vague label came back: {}",
            out.html
        );
        assert!(out.html.contains("Clear all bindings"), "{}", out.html);
        for act in [
            "restore-backup",
            "restore-defaults",
            "clear-all",
            "restore-latest",
        ] {
            assert!(
                out.html.contains(&format!(r#"data-act="{act}""#)),
                "missing {act}: {}",
                out.html
            );
        }
        // The third destination wears its timestamp (the sample slot has one).
        assert!(
            out.html
                .contains("Restore backup from 2026-08-05 14:32:07 UTC"),
            "{}",
            out.html
        );
        // The legend carries the readable truth: each zone appears a second
        // time as a legend row (same data-fn → same click action), with the
        // binding as its key tag (A→G, B→F, lx.min→M from the sample).
        assert!(out.html.contains(r#"class="lrow""#), "{}", out.html);
        assert_eq!(
            out.html.matches(r#"data-fn="A""#).count(),
            2,
            "one zone + one legend row: {}",
            out.html
        );
        assert!(out.html.contains(">G<"), "A's key tag: {}", out.html);
        assert!(out.html.contains(">F<"), "B's key tag: {}", out.html);
        assert!(out.html.contains(">M<"), "lx.min's key tag: {}", out.html);
        // Persona-aware, disambiguated legend labels: the glyph is its own
        // styled span now, the group prefix sits beside it.
        assert!(out.html.contains(">LS <"), "{}", out.html);
        assert!(out.html.contains(">D-pad <"), "{}", out.html);
        assert_eq!(
            text_in(&out.html, "lid id-dir").as_deref(),
            Some("↑"),
            "the legend leads with the same identity glyph: {}",
            out.html
        );
        // A cleared function renders the honest unbound row.
        assert!(out.html.contains("lrow l-unbound"), "{}", out.html);
        // Live mode: the hint renders, the read-only banner does not.
        assert!(out.html.contains("press the panel key"), "{}", out.html);
        assert!(!out.html.contains("read-only"), "{}", out.html);
    }

    /// FEATURE 1, Xbox. The vendored art is a line drawing with no letters on
    /// it, so every zone must say what it IS — in the canonical colours, and
    /// whether or not anything is bound to it. Victor: "I can see G is mapped
    /// to A but I can't see the A xbox button".
    #[test]
    fn every_xbox_zone_wears_its_identity_in_the_canonical_palette() {
        let out = render_map(&page(), &sample(), None);
        for (idk, glyph) in [("xa", "A"), ("xb", "B"), ("xx", "X"), ("xy", "Y")] {
            assert_eq!(
                text_in(&out.html, &format!("zid id-{idk}")).as_deref(),
                Some(glyph),
                "face button {glyph} lost its identity glyph: {}",
                out.html
            );
        }
        // Shoulders are DATA now, not CSS `content` keyed off the stage class.
        for label in ["LT", "LB", "RB", "RT"] {
            assert!(
                out.html.contains(&format!(">{label}<")),
                "{label}: {}",
                out.html
            );
        }
        assert_eq!(
            text_in(&out.html, "zid id-sh").as_deref(),
            Some("LT"),
            "{}",
            out.html
        );
        // Center cluster + sticks + dpad.
        assert!(out.html.contains(">view<"), "{}", out.html);
        assert!(out.html.contains(">menu<"), "{}", out.html);
        assert!(out.html.contains(">guide<"), "{}", out.html);
        assert_eq!(
            text_in(&out.html, "zid id-hub").as_deref(),
            Some("L3"),
            "{}",
            out.html
        );
        assert_eq!(
            text_in(&out.html, "zid id-dir").as_deref(),
            Some("↑"),
            "{}",
            out.html
        );
        // The point of the exercise: an UNBOUND control still reads as a
        // control. `start` is cleared in the sample — identity present, tag
        // empty.
        assert!(
            out.html.contains(r#"class="zone z-chip z-unbound""#),
            "{}",
            out.html
        );
    }

    /// FEATURE 1, PlayStation: the Sony glyphs in the Sony hues, and the
    /// shoulders named L1/L2 rather than LB/LT.
    #[test]
    fn every_playstation_zone_wears_the_sony_glyphs() {
        let mut payload = sample();
        payload.selected = 3;
        let out = render_map(&page(), &payload, None);
        for (idk, glyph) in [("pc", "✕"), ("po", "○"), ("pt", "△"), ("psq", "□")] {
            assert_eq!(
                text_in(&out.html, &format!("zid id-{idk}")).as_deref(),
                Some(glyph),
                "Sony glyph {glyph} missing: {}",
                out.html
            );
        }
        for label in ["L1", "L2", "R1", "R2"] {
            assert!(
                out.html.contains(&format!(">{label}<")),
                "{label}: {}",
                out.html
            );
        }
        assert!(out.html.contains(">share<"), "{}", out.html);
        assert!(out.html.contains(">PS<"), "{}", out.html);
        // …and no Xbox letters anywhere on a PlayStation slot.
        assert!(!out.html.contains(r#"id-xa"#), "{}", out.html);
    }

    /// FEATURE 3. One key driving several controls is native to the engine
    /// (docs/INPUT-TRANSFORMS.md §1a), so both readers report it as a GROUP,
    /// never as a conflict: the zone tag turns cool-toned, the legend row
    /// carries "also …", and every co-bound control names its partners.
    #[test]
    fn a_key_bound_twice_reads_as_a_group_not_a_conflict() {
        let mut payload = sample();
        // A and B and rt all on G — Victor's "one key, three controls".
        payload.mapper.slots[0]
            .bindings
            .insert("B".into(), vec!["G".into()]);
        payload.mapper.slots[0]
            .bindings
            .insert("rt".into(), vec!["G".into()]);
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains("z-shared"), "{}", out.html);
        assert!(out.html.contains("l-shared"), "{}", out.html);
        // Every co-bound control names its partners, in ZONE-TABLE order (the
        // pad's own reading order, not the order they were bound).
        assert!(out.html.contains(">also RT · B<"), "A's row: {}", out.html);
        assert!(out.html.contains(">also RT · A<"), "B's row: {}", out.html);
        assert!(out.html.contains(">also B · A<"), "RT's row: {}", out.html);
        assert!(
            out.html.contains(r#"title="G also drives RT, B""#),
            "{}",
            out.html
        );
        // Nothing about it is styled or worded as an error. Asserted against
        // the VISIBLE page: forma-ir's native props attribute carries a
        // `conflictLine` slot on every paint (ledger #8/#19), which says
        // nothing about what the user reads.
        let visible = visible(&out.html);
        assert!(!visible.contains("conflict"), "{visible}");
        // A control bound to a key nobody else uses stays plain.
        assert!(
            !out.html.contains(">also LS ◀<"),
            "lx.min is not shared: {}",
            out.html
        );
    }

    /// MANY KEYS → ONE CONTROL, the read half. The engine and the TOML have
    /// always allowed `A = ["S", "Enter"]` (docs/INPUT-TRANSFORMS.md §1a —
    /// Victor's own imported preset used it) and the page used to fold it into
    /// one tag. Now: the art shows the first key and counts the rest, the
    /// legend gives every key its own chip and its own ✕, and the tooltip
    /// names all of them AND says they are alternatives, not a chord.
    #[test]
    fn a_control_with_several_keys_shows_every_one_of_them() {
        let mut payload = sample();
        payload.mapper.slots[0]
            .bindings
            .insert("A".into(), vec!["S".into(), "Enter".into()]);
        let out = render_map(&page(), &payload, None);
        let html = &out.html;

        // The art: first key + how many more.
        assert!(html.contains(">S +1<"), "{html}");
        // Every reader names both, and neither says "S+Enter" — that reads as
        // the chord this is not.
        assert!(
            html.contains("A — S · Enter (2 keys — any one of them presses it)"),
            "{html}"
        );
        assert!(
            !html.contains("S+Enter"),
            "the chord spelling came back: {html}"
        );
        // The legend: one chip per key, each carrying its own remove payload.
        assert!(html.contains(r#"data-rmkey="A|S""#), "{html}");
        assert!(html.contains(r#"data-rmkey="A|Enter""#), "{html}");
        assert!(
            html.contains("remove S from A — it keeps Enter"),
            "the ✕ says what SURVIVES it: {html}"
        );
        // A control with one key keeps exactly one chip; the rest are off.
        assert!(html.contains(r#"data-rmkey="B|F""#), "{html}");
        assert!(html.contains(r#"class="lkc lk1 off""#), "{html}");
        assert!(html.contains("l-multi"), "{html}");
    }

    /// The chips summarize instead of growing without bound — same rule as the
    /// shared-key badge, and the tail is still named in the tooltip.
    #[test]
    fn more_keys_than_chips_are_counted_and_still_named() {
        let mut payload = sample();
        payload.mapper.slots[0].bindings.insert(
            "A".into(),
            ["S", "Enter", "Space", "J"]
                .iter()
                .map(|k| (*k).to_owned())
                .collect(),
        );
        let out = render_map(&page(), &payload, None);
        let html = &out.html;
        assert!(html.contains(">S +3<"), "the art counts them: {html}");
        assert!(html.contains(">+1<"), "the legend's tail counter: {html}");
        assert!(
            html.contains("1 more key(s): S · Enter · Space · J"),
            "the tail is still named in full: {html}"
        );
    }

    /// Two controls share when their key SETS INTERSECT. The old rule compared
    /// the joined tags, so a control that grew a second key silently stopped
    /// reporting the fan-out it was still part of.
    #[test]
    fn a_multi_key_control_still_reports_the_key_it_shares() {
        let mut payload = sample();
        // A has G (as the sample binds it) plus Enter; B has G alone.
        payload.mapper.slots[0]
            .bindings
            .insert("A".into(), vec!["G".into(), "Enter".into()]);
        payload.mapper.slots[0]
            .bindings
            .insert("B".into(), vec!["G".into()]);
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains(">also B<"), "A's row: {}", out.html);
        assert!(out.html.contains(">also A<"), "B's row: {}", out.html);
    }

    /// The badge summarizes instead of growing without bound.
    #[test]
    fn a_key_on_many_controls_summarizes_the_tail() {
        let names: Vec<String> = ["A", "B", "X", "Y", "RT"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(share_text(&names[..1]), "also A");
        assert_eq!(share_text(&names), "also A · B · X +2");
        assert_eq!(share_text(&[]), "");
        assert_eq!(share_title("G", &names[..2]), "G also drives A, B");
    }

    /// `?slot=3` selects the PlayStation slot: DS4 art, Sony labels.
    #[test]
    fn selected_slot_drives_art_and_labels() {
        let mut payload = sample();
        payload.selected = 3;
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html.contains("P3 · PlayStation (DS4) · IPAC P3"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"src="/_assets/pad-ds4.svg""#),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"src="/_assets/pad-xbox.svg""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("options"),
            "Sony start label: {}",
            out.html
        );
    }

    /// No daemon: read-only with the reason AND the prefilled CLI line —
    /// never a dead-looking page.
    #[test]
    fn an_unreachable_daemon_renders_read_only_with_the_cli_fallback() {
        let mut payload = sample();
        payload.session = SessionView::unreachable("no daemon control channel");
        payload.learn = LearnView::unavailable("no daemon control channel");
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html.contains("read-only: no daemon control channel"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("ksx map --preset \"IPAC P1\" --function"),
            "{}",
            out.html
        );
        // Zones still render (read-only browsing), bindings included.
        assert!(out.html.contains(r#"data-fn="A""#), "{}", out.html);
        assert!(out.html.contains(">G<"), "{}", out.html);
        // The preset-actions card renders inert, never hidden.
        assert!(
            out.html.contains(r#"class="card pactions off""#),
            "{}",
            out.html
        );
    }

    /// FIX 0. A running session still cannot be mapped — the daemon's refusal
    /// is deliberate (daemon/pipe.rs writes out why) — but the page turns it
    /// into ONE CLICK instead of a dead end: the banner explains the capture,
    /// and "Pause emulation & map" is right there.
    #[test]
    fn a_running_session_offers_pause_and_map_instead_of_a_dead_end() {
        let mut payload = sample();
        payload.session.running = true;
        payload.session.line = "running — 4 pad(s)".into();
        payload.session.profile = Some("Steam".into());
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html
                .contains("Emulation is running: panel keys are captured"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("Pause emulation &amp; map"),
            "{}",
            out.html
        );
        assert!(out.html.contains(r#"data-act="pause-map""#), "{}", out.html);
        // …and every control looks as inert as it is, without being a
        // `disabled` button that would swallow the click that explains why.
        assert!(out.html.contains("z-dead"), "{}", out.html);
        assert!(out.html.contains("l-dead"), "{}", out.html);
        // The ATTRIBUTE, specifically: `MacroView.disabled` (a macro switched
        // off) is a legitimate word in the island's JSON props and must not be
        // mistaken for a dead control.
        assert!(!out.html.contains(" disabled>"), "{}", out.html);
        assert!(!out.html.contains(" disabled="), "{}", out.html);
        assert!(!out.html.contains(" disabled "), "{}", out.html);
    }

    /// A macro switched OFF has to say so where the eye lands — the head line
    /// and the switch itself — because every other line on the card describes
    /// a sequence that will not run.
    #[test]
    fn a_disabled_macro_says_so_on_the_card_and_on_its_switch() {
        let mut payload = sample();
        payload.macros.macros[0].disabled = true;
        let mac = selected_macro(&payload);
        let slots = scalar_slots(&payload, payload.mapper.slots.first(), mac.as_ref(), None);

        let head = slots["macroHead"].as_str().unwrap();
        assert!(head.contains("DISABLED"), "{head}");
        // ...and it names what SURVIVES, so "disabled" never reads as "gone".
        assert!(head.contains("keeps its steps"), "{head}");
        assert_eq!(slots["macroEnableCls"], "btn btn-mini macen offstate");
        // The button reads as the STATE, not the action.
        assert!(
            slots["macroEnableLabel"]
                .as_str()
                .unwrap()
                .contains("DISABLED"),
            "{slots}"
        );

        // An enabled macro is quiet on both.
        let slots = scalar_slots(&sample(), None, Some(&hadouken()), None);
        assert!(!slots["macroHead"].as_str().unwrap().contains("DISABLED"));
        assert_eq!(slots["macroEnableCls"], "btn btn-mini macen on");
        assert_eq!(slots["macroEnableLabel"], "Enabled");
        // No macro at all: an inert switch, not one for a table that is absent.
        let slots = scalar_slots(&sample(), None, None, None);
        assert_eq!(slots["macroEnableCls"], "btn btn-mini macen off dead");
    }

    /// The SLOT's master switch: a sentence naming the file to edit, because
    /// Studio has no config writer and a control that did nothing would be
    /// worse than none. Silent on every slot that runs macros.
    #[test]
    fn a_slot_with_macros_off_says_which_line_to_change() {
        let mut payload = sample();
        payload.mapper.slots[0].macros_off = true;
        let line = slot_macros_line(payload.mapper.slots.first());
        for part in [
            "Slot 1",
            "macros = \"off\"",
            "TOURNAMENT",
            "nothing is deleted",
            "config.toml",
            "games.toml",
        ] {
            assert!(line.contains(part), "{line}");
        }
        // ...and it reaches the page.
        let slots = scalar_slots(
            &payload,
            payload.mapper.slots.first(),
            Some(&hadouken()),
            None,
        );
        assert_eq!(slots["slotMacrosLine"], line);

        // The ordinary slot says nothing at all — an empty string renders as
        // no element, which is the honest amount of screen for "as usual".
        assert_eq!(slot_macros_line(sample().mapper.slots.first()), "");
        assert_eq!(slot_macros_line(None), "");
    }

    /// FIX 1, the headline case: Victor quit the tray daemon and then clicked
    /// controls that silently did nothing. The banner has to be the FIRST
    /// thing in <main>, name the split (read works, write does not) and print
    /// the exact command — profile flag included, because plain `ksx daemon`
    /// refuses to start on a games.toml cabinet.
    #[test]
    fn an_unreachable_daemon_shouts_at_the_top_of_the_page_with_the_command() {
        let mut payload = sample();
        payload.session = SessionView {
            profile: Some("Steam".into()),
            ..SessionView::unreachable("no daemon control channel")
        };
        payload.learn = LearnView::unavailable("no daemon control channel");
        let out = render_map(&page(), &payload, None);

        assert!(
            out.html.contains(crate::render::NO_DAEMON_HEADLINE),
            "the banner headline drifted from NO_DAEMON_HEADLINE: {}",
            out.html
        );
        assert!(
            out.html.contains("ksx daemon --game &quot;Steam&quot;")
                || out.html.contains(r#"ksx daemon --game "Steam""#),
            "the command must carry the profile flag: {}",
            out.html
        );
        assert!(out.html.contains("tray icon"), "{}", out.html);
        // Unmissable means BEFORE the content it is about.
        let banner = out
            .html
            .find(crate::render::NO_DAEMON_HEADLINE)
            .expect("banner present");
        let stage = out.html.find("stagecard").expect("stage present");
        assert!(
            banner < stage,
            "the banner is buried below the controller art"
        );
        // Controls that cannot work look like it, on both readers.
        assert!(out.html.contains("z-dead"), "{}", out.html);
        assert!(out.html.contains("l-dead"), "{}", out.html);
    }

    /// No backup on disk = no "Restore backup from …" button. Offering a road
    /// home that is not there is worse than not offering one.
    #[test]
    fn the_backup_restore_button_appears_only_when_a_backup_exists() {
        let mut payload = sample();
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains(r#"data-act="restore-latest""#));

        for slot in &mut payload.mapper.slots {
            slot.backup = None;
        }
        let out = render_map(&page(), &payload, None);
        assert!(
            !out.html.contains(r#"data-act="restore-latest""#),
            "{}",
            out.html
        );
        // …while the two that always exist stay.
        assert!(out.html.contains(r#"data-act="restore-backup""#));
        assert!(out.html.contains(r#"data-act="restore-defaults""#));
    }

    /// Auto-save is only reassuring if the page says so in words — and since
    /// v8 the same paragraph has to state the OTHER half of the bargain: no
    /// action asks "are you sure?", because each one reports itself with an
    /// Undo, and the restore options are the wider road home.
    #[test]
    fn the_preset_card_states_the_save_model_plainly() {
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html.contains("Every binding saves immediately"),
            "{}",
            out.html
        );
        assert!(out.html.contains("Undo"), "{}", out.html);
        assert!(
            out.html.contains("restore options"),
            "the wider road home went unmentioned: {}",
            out.html
        );
        assert!(
            out.html.contains("timestamped backup"),
            "the promise that makes an optimistic write safe: {}",
            out.html
        );
    }

    /// v8: the toast stack is CLIENT-only. Its container ships (with the
    /// list's markers inside, which is what lets the adoption path insert
    /// into it), but a server render has taken no action, so it must never
    /// paint a toast — and the no-JS flash line it replaced still exists for
    /// a page with no JavaScript at all.
    #[test]
    fn the_toast_stack_ships_empty_and_the_no_js_flash_line_survives() {
        let out = render_map(&page(), &sample(), None);
        let at = out
            .html
            .find(r#"class="toasts""#)
            .unwrap_or_else(|| panic!("the toast stack is missing: {}", out.html));
        // The empty list still emits its `<!--f:lN-->` markers, and the
        // client's adoption path inserts BEFORE the closing one. No markers =
        // a stack that silently never shows a toast.
        let container = &out.html[at..];
        let container = &container[..container.find("</div>").unwrap_or(container.len())];
        assert!(
            container.contains("<!--f:l"),
            "the toast list emitted no markers to adopt: {container}"
        );
        assert!(
            !out.html.contains(r#"class="toast toast-"#),
            "a toast SSR'd: {}",
            out.html
        );
        assert!(
            !out.html.contains(r#"data-undo="#),
            "an Undo button SSR'd where nothing has happened: {}",
            out.html
        );
        // The server-rendered flash channel (savedLine + its two shows) is
        // still part of the seam — that is the no-JS page's only feedback.
        assert!(
            scalar_slots(&sample(), None, Some(&hadouken()), None)
                .as_object()
                .unwrap()
                .contains_key("savedLine"),
            "the SSR flash slot was dropped"
        );
    }

    /// A daemon that predates the learn verbs: honest reason with the pipe's
    /// own error text.
    #[test]
    fn a_pre_mapper_daemon_is_reported_not_hidden() {
        let mut payload = sample();
        payload.learn = LearnView::unavailable("unknown verb 'learn-poll'");
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html.contains("does not answer the learn verbs"),
            "{}",
            out.html
        );
        assert!(out.html.contains("unknown verb"), "{}", out.html);
    }

    /// Ledger #5 parity, mapper edition: the embedded payload block IS the
    /// /api/map payload, and the ledger-#8 workaround is gone.
    #[test]
    fn the_payload_block_matches_the_api_map_payload_shape() {
        let payload = sample();
        let json = crate::render::payload_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("payload parse");
        assert_eq!(
            parsed,
            serde_json::to_value(&payload).unwrap(),
            "the payload block must be byte-compatible with /api/map"
        );
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains(&json), "{}", out.html);
        // The `__forma_islands` block is UPSTREAM's again, not ours.
        //
        // ksx used to hand-emit one, impersonating the islands protocol; that
        // workaround died when compiler 0.3.1 populated `slot_ids` (ledger
        // #8), and this asserted the block was gone. At forma-ir 0.2.0 the
        // walker emits it itself for props over `INLINE_PROPS_MAX_BYTES`
        // (1 KiB) — upstream acting on ksx's finding #22, that inline props
        // were a third of this very response and could not be switched off.
        // /map's props are the largest on the site, so the block is expected
        // here. What must stay true is that ksx does not write one: exactly
        // one, emitted by the walker.
        assert_eq!(
            out.html.matches(r#"id="__forma_islands""#).count(),
            1,
            "the walker emits the shared props block; a second one would be a \
             duplicate DOM id and would shadow it: {}",
            out.html
        );
        // Native island props (ledger #8 closed): the walker's own emission,
        // carrying the rendered slot values the client adopts against —
        // delivered through the shared block on this page, since /map's props
        // are far over the 1 KiB inline ceiling. Pin the VALUES, not the
        // channel, so a payload that shrinks below the ceiling and goes back
        // inline is not a test failure.
        assert!(
            out.html.contains(r#""actionsCls":"#) || out.html.contains("&quot;actionsCls&quot;:"),
            "the island must receive its rendered slot values: {}",
            out.html
        );
    }

    /// Dogfood ledger #20, both halves, pinned server-side.
    ///
    /// **(a) attribute position.** A `"a" + "b"` as an h() ATTRIBUTE value
    /// used to compile to an empty slot: the attribute was emitted with NO
    /// VALUE AT ALL. Both 360 buttons shipped without a tooltip that way. The
    /// titles below are concatenations again (compiler 0.3.1 folds them), and
    /// this asserts the folded text really reaches the SSR markup — the
    /// failure mode is silent, so only an assertion catches it.
    ///
    /// **(b) child position.** A `"a" + "b"` as a CHILD used to be a
    /// BinaryExpression the walker could not fold, and it emitted an
    /// anonymous SECOND ISLAND. `.macstephint` is one concatenated child
    /// again; the single-island assertion in
    /// `embedded_map_ir_slot_layout_matches_the_seam` is the other half of
    /// this guard.
    #[test]
    fn concatenated_strings_render_in_attributes_and_children() {
        let out = render_map(&page(), &sample(), None);
        // (a) The two ends of a folded attribute concatenation, on the button
        // the pwtest suite reads the title back from.
        assert!(
            out.html.contains(
                r#"title="append a quarter-circle forward: ↓ ↘ → — three steps, the middle one the diagonal (each row spells the pair it stores)""#
            ),
            "folded attribute concatenation missing from SSR: {}",
            out.html
        );
        assert!(
            out.html.contains(
                r#"title="append a full 360 (spinning piledriver), clockwise from →: → ↘ ↓ ↙ ← ↖ ↑ ↗ — eight steps, four of them diagonals""#
            ),
            "{}",
            out.html
        );
        assert!(
            out.html
                .contains("title=\"switch this macro off (or back on) without losing it — the steps and the key that starts it stay exactly where they are."),
            "{}",
            out.html
        );
        // This used to assert `title=""` appeared NOWHERE, as a proxy for "the
        // concatenated attribute did not come out empty". forma-ir 0.2.0
        // retired that proxy, and correctly:
        //
        //   "DYN_ATTR treats an empty string as a value. value=""/alt="" now
        //    render as attr="" instead of being dropped, matching what the
        //    client's setAttribute produces for the same slot."
        //
        // ksx has attributes that are DELIBERATELY empty — `chipTitle` returns
        // "" for an unbound key chip, because there is nothing to say about a
        // key that is not there — so an unbound chip now ships `title=""`.
        // That is SSR and hydration agreeing where they used to differ
        // silently, which is the opposite of a bug.
        //
        // So pin the property the ledger actually cares about, on a row whose
        // title is built by concatenation and is never legitimately empty: the
        // macro Enable switch above, and a bound key chip here. An empty one
        // of THESE would be the regression.
        let visible = visible(&out.html);
        assert!(
            !visible.contains(r#"class="lkc lk1" title="""#),
            "a BOUND key chip's concatenated title must not be empty: {visible}"
        );
        // (b) The concatenated CHILD renders as one uninterrupted string.
        assert!(
            out.html.contains(
                "…or ＋↑ / ＋↓ on a row to insert next to it. ✕ deletes a step — on the last one it empties it instead, because a macro with no steps is not something ksx can save."
            ),
            "folded child concatenation missing from SSR: {}",
            out.html
        );
    }

    /// The attribution promised in studio-ui/art/README.md is visibly on the
    /// page (both pages carry it; render.rs pins the status page).
    #[test]
    fn the_mapper_page_credits_the_controller_art() {
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html.contains("Gamepad-Asset-Pack (MIT) by AL2009man"),
            "{}",
            out.html
        );
        // The nav is STATIC MARKUP duplicated per island, so a new screen is
        // invisible until every existing one links to it. Asserted here, on
        // this page, because a dropped link is otherwise a silent regression.
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/setup""#), "{}", out.html);
    }

    /// The nav rail reaches every screen from here.
    ///
    /// It is STATIC MARKUP inside this page's own island — there is no shared
    /// nav component and nothing server-injected — so a new page is invisible
    /// from the mapper until `MapIsland.ts` is edited, with nothing anywhere to
    /// report the omission. This test is that report.
    #[test]
    fn the_nav_reaches_every_screen() {
        let out = render_map(&page(), &sample(), None);
        for href in [
            r#"href="/""#,
            r#"href="/pads""#,
            r#"href="/devices""#,
            r#"href="/profiles""#,
            r#"href="/setup""#,
        ] {
            assert!(out.html.contains(href), "missing {href}: {}", out.html);
        }
        assert!(
            out.html.contains(r#"aria-current="page""#),
            "the current screen must be marked: {}",
            out.html
        );
    }

    /// v9, the headline: with JavaScript switched off the page is still a
    /// working mapper. Every control owns a real `<form method="post">` — a
    /// key picker, a Bind submit and a Clear submit — and every other action
    /// on the page (the four preset writes, the pause, the slot strip) is a
    /// form or a link too. No `data-act` button is left as the only way to
    /// do anything.
    #[test]
    fn every_mapper_action_has_a_no_js_form_or_link() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;

        // One row form per mappable control, plus the bind-by-name panel.
        assert_eq!(
            html.matches(r#"class="lrowwrap""#).count(),
            25,
            "one wrapper per legend row: {html}"
        );
        assert_eq!(
            html.matches(r#"action="/map/bind""#).count(),
            27,
            "25 row forms + the bind-by-name panel + v11's macro trigger: {html}"
        );
        // The row form carries the slot (the server resolves the preset from
        // it), the function, a key select and both verbs. Clear rides the
        // same form through formaction — one form, two destinations.
        assert!(
            html.contains(r#"<input type="hidden" name="slot" value="1">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<input type="hidden" name="function" value="A">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<select class="keysel" name="key""#),
            "{html}"
        );
        assert_eq!(
            html.matches(r#"formaction="/map/clear""#).count(),
            27,
            "every bind form offers Clear without JavaScript — the macro trigger included, \
             which is why it is 27 and the add/remove pair below is 26: those two are
             read-modify-write against the `bindings` map, and macro triggers do not live \
             there: {html}"
        );
        // v10: the same form, two more verbs — ADD the picked key to what the
        // control has, or REMOVE just that one. Removing one key of several
        // without JavaScript needs to name WHICH key, and the row's own
        // picker is that name (no per-key form, 25 rows over).
        for action in ["/map/add", "/map/key/remove"] {
            assert_eq!(
                html.matches(&format!(r#"formaction="{action}""#)).count(),
                26,
                "25 row forms + the bind-by-name panel offer {action}: {html}"
            );
        }
        // The four preset writes and the pause are forms, not bare buttons.
        for (action, mode) in [
            ("/map/preset/clear-all", None),
            ("/map/preset/restore", Some("session-backup")),
            ("/map/preset/restore", Some("latest-backup")),
            ("/map/preset/restore", Some("defaults")),
        ] {
            assert!(
                html.contains(&format!(r#"method="post" action="{action}""#)),
                "{action} is not a form: {html}"
            );
            if let Some(mode) = mode {
                assert!(
                    html.contains(&format!(r#"name="mode" value="{mode}""#)),
                    "{mode}: {html}"
                );
            }
        }
        // Slot switching is a link — `?slot=N` was always a route.
        assert!(html.contains(r#"href="/map?slot=2""#), "{html}");
        assert!(html.contains(r#"href="/map?slot=3""#), "{html}");
        // …and the JS hooks survive on the same elements, so the island can
        // still intercept every one of them.
        assert!(html.contains(r#"data-slot="2""#), "{html}");
        assert!(html.contains(r#"data-act="clear-all""#), "{html}");
    }

    /// The no-JS select must offer EVERY key a preset can hold — the whole
    /// point of picking instead of learning. The vocabulary lives in the
    /// TypeScript twin (MapPage.ts), so this test reads the one true source,
    /// `Key::ALL`, and checks the rendered page against it: a key added to
    /// ksx-core and not to the page fails here rather than silently becoming
    /// unbindable without a shell.
    ///
    /// An `<option>` with no `value` attribute submits its own text, which is
    /// why the assertion is on the text and not on an attribute.
    #[test]
    fn every_bindable_key_name_is_offered_by_the_no_js_selects() {
        use ksx_core::key::Key;
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        for key in Key::ALL {
            let name = key.name();
            let offered = html.contains(&format!("<option>{name}</option>"));
            // The three exclusions, stated as an assertion rather than a
            // comment: the inert placeholder (Clear is how you unbind), the
            // sentinel, and the mouse pseudo-keys no capture path produces.
            let excluded = matches!(*key, Key::None | Key::Unknown) || key.is_mouse_pseudo();
            assert_eq!(
                offered, !excluded,
                "key {name}: offered={offered}, expected={}",
                !excluded
            );
        }
        // Every mappable FUNCTION is pickable too, canonical spelling.
        for z in ZONE_XBOX.iter() {
            assert!(
                html.contains(&format!("<option>{}</option>", z.fn_name)),
                "function {} is not in the bind-by-name picker: {html}",
                z.fn_name
            );
        }
    }

    /// The post-redirect-get seam: `/map?flash=…` renders the outcome in the
    /// SSR flash line, ok and error styled apart — the only feedback channel
    /// a page without JavaScript has. (With JavaScript, map.ts re-reports it
    /// as a toast and blanks this, so nothing is said twice.)
    #[test]
    fn a_flash_query_renders_the_server_side_outcome() {
        let out = render_map(&page(), &sample(), Some("A is now G."));
        assert!(
            out.html.contains(r#"class="flash flash-ok""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("A is now G."), "{}", out.html);
        assert!(
            !out.html.contains(r#"class="flash flash-err""#),
            "{}",
            out.html
        );

        let out = render_map(&page(), &sample(), Some("error: the daemon refused"));
        assert!(
            out.html.contains(r#"class="flash flash-err""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("error: the daemon refused"),
            "{}",
            out.html
        );

        // No flash, no line: a fresh GET has taken no action.
        let out = render_map(&page(), &sample(), None);
        assert!(!out.html.contains("flash flash-ok"), "{}", out.html);
        assert!(!out.html.contains("flash flash-err"), "{}", out.html);
    }

    /// `writable` is deliberately WIDER than `learnable`: the no-JS forms
    /// pick a key rather than listening for one, so they work while a session
    /// runs (a binding edit is hot-swapped) and on a daemon too old to know
    /// the learn verbs. Only a missing daemon dims them — and even then they
    /// are still submittable, because a refusal that names the reason beats a
    /// control that is not there.
    #[test]
    fn the_no_js_forms_stay_live_wherever_a_write_can_land() {
        let mut running = sample();
        running.session.running = true;
        let out = render_map(&page(), &running, None);
        assert!(out.html.contains(r#"class="lbind nojs""#), "{}", out.html);
        assert!(out.html.contains("z-dead"), "the LEARN path is still off");

        let mut old = sample();
        old.learn = LearnView::unavailable("unknown verb 'learn-poll'");
        let out = render_map(&page(), &old, None);
        assert!(out.html.contains(r#"class="lbind nojs""#), "{}", out.html);

        let mut dead = sample();
        dead.session = SessionView::unreachable("no daemon control channel");
        dead.learn = LearnView::unavailable("no daemon control channel");
        let out = render_map(&page(), &dead, None);
        assert!(
            out.html.contains(r#"class="lbind nojs off""#),
            "no daemon = the inert look, never a removed form: {}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/map/bind""#),
            "the form itself must survive: {}",
            out.html
        );
    }

    // ── v11: the macro editor ──────────────────────────────────────────────

    /// The sampling rule is ksx-core's, not this crate's. ksx-studio depends
    /// on no ksx crate at runtime, so the floor and the frame conversion are
    /// MIRRORED here — and a mirror that drifts is exactly the bug this test
    /// exists to prevent (a grid that says "fine" about a step the engine
    /// raises, or vice versa).
    #[test]
    fn the_sampling_floor_and_frame_maths_match_ksx_core() {
        use ksx_core::macros::{MacroStep, StepDuration, MIN_STEP_MS as CORE_MIN};
        assert_eq!(MIN_STEP_MS, CORE_MIN);
        for frames in [1u32, 2, 3, 4, 60] {
            assert_eq!(
                frames_ms(frames),
                StepDuration::Frames(frames).ms(),
                "{frames}"
            );
        }
        for (ms, allow_short) in [(5u32, false), (5, true), (33, false), (500, false)] {
            let mine = step(&[], Some(ms), None, allow_short);
            let theirs = if allow_short {
                MacroStep::short(Vec::new(), ms)
            } else {
                MacroStep::new(Vec::new(), ms)
            };
            assert_eq!(
                effective_ms(&mine),
                theirs.effective_ms(),
                "{ms}/{allow_short}"
            );
            assert_eq!(step_warning(&mine).is_empty(), !theirs.is_short());
            assert_eq!(step_warning_long(&mine).is_empty(), !theirs.is_short());
        }
        // A frames-authored step goes through the same floor.
        assert_eq!(effective_ms(&step(&[], None, Some(1), false)), MIN_STEP_MS);
        assert_eq!(effective_ms(&step(&[], None, Some(3), false)), 50);
        // Total run length is the sum of the EFFECTIVE durations, like
        // `Macro::total_ms` — a raised step moves everything after it.
        assert_eq!(total_ms(&hadouken()), 200);
    }

    /// THE SEAM that keeps the lens honest: this crate's `fold` — which works
    /// in function-name strings, because ksx-studio links no ksx crate at
    /// runtime — must agree with `ksx_core::diagonal::fold`, which works in
    /// `Binding`s, on every hold either of them can be shown.
    ///
    /// The two are re-derivations of ONE rule, the way the zone tables and the
    /// sampling floor already are. A drift here is what "the grid says ↘ and
    /// the validator says something else" would look like, and it fails the
    /// build instead.
    #[test]
    fn the_diagonal_lens_matches_ksx_core() {
        use ksx_config::parse_function;
        use ksx_core::diagonal as core;
        use ksx_core::DirMechanism;

        let as_core = |m: Mechanism| match m {
            Mechanism::Dpad => DirMechanism::Dpad,
            Mechanism::LeftStick => DirMechanism::LeftStick,
            Mechanism::RightStick => DirMechanism::RightStick,
        };
        // The vocabulary a step can hold: every direction of every mechanism,
        // a hand-written partial deflection, a centred axis, a button, and a
        // name the file may carry that this page cannot parse.
        let vocabulary = [
            "dpad.up",
            "dpad.down",
            "dpad.left",
            "dpad.right",
            "ly.max",
            "ly.min",
            "lx.min",
            "lx.max",
            "ly.-16384",
            "lx.0",
            "ry.max",
            "rx.min",
            "A",
            "lt",
            "consume",
        ];
        for a in vocabulary {
            for b in vocabulary {
                for c in vocabulary {
                    let hold = [a.to_owned(), b.to_owned(), c.to_owned()];
                    let mine = fold(&hold);
                    let bindings: Vec<_> = hold
                        .iter()
                        .map(|f| parse_function(f).expect("vocabulary parses"))
                        .collect();
                    let theirs = core::fold(&bindings);
                    assert_eq!(mine.len(), theirs.len(), "{hold:?}");
                    for (m, t) in mine.iter().zip(theirs.iter()) {
                        match (m, t) {
                            (
                                Held::Diagonal {
                                    diag,
                                    mechanisms,
                                    members,
                                    exact,
                                },
                                core::Held::Diagonal {
                                    diag: cd,
                                    mechanisms: cm,
                                    members: cmem,
                                    exact: ce,
                                },
                            ) => {
                                assert_eq!(diag.glyph(), cd.glyph(), "{hold:?}");
                                assert_eq!(diag.numpad(), cd.numpad(), "{hold:?}");
                                assert_eq!(diag.words(), cd.words(), "{hold:?}");
                                assert_eq!(diag.move_list(), cd.move_list(), "{hold:?}");
                                assert_eq!(diag.halves(), cd.halves(), "{hold:?}");
                                assert_eq!(
                                    mechanisms.iter().copied().map(as_core).collect::<Vec<_>>(),
                                    *cm,
                                    "{hold:?}"
                                );
                                assert_eq!(members, cmem, "{hold:?}");
                                assert_eq!(exact, ce, "{hold:?}");
                            }
                            (Held::Plain { member }, core::Held::Plain { member: cmember }) => {
                                assert_eq!(member, cmember, "{hold:?}");
                            }
                            _ => panic!("{hold:?}: {m:?} vs {t:?}"),
                        }
                    }
                }
            }
        }
        // An UNPARSEABLE name is a passenger on this side too (ksx-core never
        // sees one — the loader would have rejected it — but the page does, and
        // validation already reports it).
        assert_eq!(
            fold(&["nonsense".to_owned(), "dpad.down".to_owned()]),
            vec![Held::Plain { member: 0 }, Held::Plain { member: 1 }]
        );
        // And what a PICK writes is what ksx-core says the pair is.
        for mechanism in Mechanism::ALL {
            for diag in Diag::ALL {
                let (up, right) = diag.halves();
                let mine = [
                    mechanism.function(true, up),
                    mechanism.function(false, right),
                ];
                let core_pair = core::members_of(
                    match diag {
                        Diag::UpLeft => core::Diag::UpLeft,
                        Diag::UpRight => core::Diag::UpRight,
                        Diag::DownLeft => core::Diag::DownLeft,
                        Diag::DownRight => core::Diag::DownRight,
                    },
                    as_core(mechanism),
                );
                let theirs: Vec<String> = core_pair.iter().map(ksx_config::function_name).collect();
                assert_eq!(mine.to_vec(), theirs, "{mechanism:?} {diag:?}");
            }
        }
    }

    /// ⚠ THE SIGN TRAP. "Up" on a stick is `ly.max`, NOT `ly.min` (XInput's
    /// positive Y is up). A copy-paste of the down-diagonals with the sign
    /// wrong yields an ↖ that reads back perfectly in every reader on this page
    /// and does nothing in the game — the worst failure this feature has,
    /// because nothing else catches it.
    ///
    /// So all four diagonals are asserted NAME BY NAME, on all three
    /// mechanisms, rather than only through the round trip (which a
    /// consistently-mirrored sign would pass).
    #[test]
    fn all_four_diagonals_deflect_the_way_they_are_drawn() {
        // The vertical half of an UP diagonal is the MAX end of the Y axis.
        assert_eq!(Mechanism::LeftStick.function(true, true), "ly.max");
        assert_eq!(Mechanism::LeftStick.function(true, false), "ly.min");
        assert_eq!(Mechanism::RightStick.function(true, true), "ry.max");
        assert_eq!(Mechanism::RightStick.function(true, false), "ry.min");
        assert_eq!(Mechanism::Dpad.function(true, true), "dpad.up");
        assert_eq!(Mechanism::Dpad.function(true, false), "dpad.down");

        // Every diagonal, on every mechanism, written out.
        let expected = [
            (Diag::UpLeft, Mechanism::Dpad, ["dpad.up", "dpad.left"]),
            (Diag::UpRight, Mechanism::Dpad, ["dpad.up", "dpad.right"]),
            (Diag::DownLeft, Mechanism::Dpad, ["dpad.down", "dpad.left"]),
            (
                Diag::DownRight,
                Mechanism::Dpad,
                ["dpad.down", "dpad.right"],
            ),
            (Diag::UpLeft, Mechanism::LeftStick, ["ly.max", "lx.min"]),
            (Diag::UpRight, Mechanism::LeftStick, ["ly.max", "lx.max"]),
            (Diag::DownLeft, Mechanism::LeftStick, ["ly.min", "lx.min"]),
            (Diag::DownRight, Mechanism::LeftStick, ["ly.min", "lx.max"]),
            (Diag::UpLeft, Mechanism::RightStick, ["ry.max", "rx.min"]),
            (Diag::UpRight, Mechanism::RightStick, ["ry.max", "rx.max"]),
            (Diag::DownLeft, Mechanism::RightStick, ["ry.min", "rx.min"]),
            (Diag::DownRight, Mechanism::RightStick, ["ry.min", "rx.max"]),
        ];
        for (diag, mechanism, pair) in expected {
            let (up, right) = diag.halves();
            assert_eq!(
                [
                    mechanism.function(true, up),
                    mechanism.function(false, right)
                ],
                pair,
                "{diag:?} on {mechanism:?} writes the wrong pair"
            );
            // …and reads back as ITSELF, on that mechanism and no other.
            let hold: Vec<String> = pair.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(
                fold(&hold),
                vec![Held::Diagonal {
                    diag,
                    mechanisms: vec![mechanism],
                    members: vec![0, 1],
                    exact: true,
                }],
                "{diag:?} on {mechanism:?} does not read back"
            );
        }

        // THE FULL CIRCLE, which is why all four are first class: a 360 walks
        // every position of the gate, and four of the eight are diagonals. Each
        // one has to be a column you can point at, or the feature misses its
        // purpose.
        let gate = [
            (vec!["dpad.right"], "D-pad →"),
            (vec!["dpad.down", "dpad.right"], "D-pad ↘"),
            (vec!["dpad.down"], "D-pad ↓"),
            (vec!["dpad.down", "dpad.left"], "D-pad ↙"),
            (vec!["dpad.left"], "D-pad ←"),
            (vec!["dpad.up", "dpad.left"], "D-pad ↖"),
            (vec!["dpad.up"], "D-pad ↑"),
            (vec!["dpad.up", "dpad.right"], "D-pad ↗"),
        ];
        let mut payload = sample();
        payload.macros.macros[0].steps = gate
            .iter()
            .map(|(hold, _)| step(hold, Some(50), None, false))
            .collect();
        let out = render_map(&page(), &payload, None);
        for (hold, words) in gate {
            assert!(
                out.html.contains(&format!(">{words}<")),
                "the 360's {hold:?} step does not read as {words}: {}",
                out.html
            );
        }
        // Four of the eight are diagonals, each lit on its own column.
        assert_eq!(
            out.html.matches(r#"class="maccell on isdiag""#).count(),
            4,
            "a 360 has four diagonal steps: {}",
            out.html
        );
        for diag in Diag::ALL {
            assert!(
                out.html
                    .contains(&format!(r#"|{}""#, diag_token(Mechanism::Dpad, diag))),
                "{diag:?} has no column: {}",
                out.html
            );
        }
    }

    /// The grid is three RINGS, in ring order, under a group band — and every
    /// diagonal is a column you can point at.
    #[test]
    fn the_grid_is_three_rings() {
        for persona in ["xbox360", "playstation"] {
            let columns = macro_columns(persona);
            assert_eq!(
                columns.len(),
                MACRO_COLUMN_COUNT,
                "{persona}: 25 zones → 37 columns"
            );
            // Every mappable function still has exactly one column…
            for z in zones_for(persona).iter() {
                assert_eq!(
                    columns.iter().filter(|c| c.token == z.fn_name).count(),
                    1,
                    "{persona}: {} lost its column",
                    z.fn_name
                );
            }
            // …plus twelve new picks nobody has had before.
            let diagonals: Vec<&str> = columns
                .iter()
                .filter(|c| c.token.starts_with("diag:"))
                .map(|c| c.token.as_str())
                .collect();
            assert_eq!(diagonals.len(), 12, "{persona}: {diagonals:?}");
            for mechanism in Mechanism::ALL {
                for diag in Diag::ALL {
                    assert!(
                        diagonals.contains(&diag_token(mechanism, diag).as_str()),
                        "{persona}: {mechanism:?} {diag:?}"
                    );
                }
            }
            // RING ORDER — ↑ ↖ ← ↙ ↓ ↘ → ↗ — is what makes a motion a shape.
            let glyphs: Vec<&str> = columns
                .iter()
                .filter(|c| c.idcls.starts_with("maccolid ") || c.token.starts_with("diag:"))
                .map(|c| c.glyph.as_str())
                .collect();
            assert_eq!(
                glyphs,
                ["↑", "↖", "←", "↙", "↓", "↘", "→", "↗"].repeat(3),
                "{persona}"
            );
            // The band, and the fact it exists for: three identical rings.
            let bands = macro_groups(Some(&slot(1, persona, "p")));
            let SlotValue::Array(bands) = bands else {
                panic!("bands are a list")
            };
            let labels: Vec<String> = bands
                .iter()
                .map(|b| match b {
                    SlotValue::Object(fields) => match &fields[0].1 {
                        SlotValue::Text(t) => t.clone(),
                        other => panic!("{other:?}"),
                    },
                    other => panic!("{other:?}"),
                })
                .collect();
            assert_eq!(
                labels,
                [
                    "SHOULDERS",
                    "FACE",
                    "SYSTEM",
                    "LEFT STICK",
                    "D-PAD",
                    "RIGHT STICK"
                ],
                "{persona}"
            );
            // Spans sum to the column count, or the header would not line up
            // with the matrix under it.
            let spans: usize = bands
                .iter()
                .map(|b| match b {
                    SlotValue::Object(fields) => match &fields[1].1 {
                        SlotValue::Text(cls) => cls
                            .rsplit_once('g')
                            .and_then(|(_, n)| n.parse::<usize>().ok())
                            .expect("g<N>"),
                        other => panic!("{other:?}"),
                    },
                    other => panic!("{other:?}"),
                })
                .sum();
            assert_eq!(spans, MACRO_COLUMN_COUNT, "{persona}");
        }
    }

    /// THE ROUND TRIP, which is the whole promise: a step nobody made through
    /// this page — hand-written into the TOML, or imported, or generated by the
    /// motion helpers — DISPLAYS as the diagonal.
    #[test]
    fn a_hand_written_pair_reads_back_as_the_diagonal() {
        let cases = [
            // The canonical pair, on each mechanism.
            (vec!["dpad.down", "dpad.right"], "D-pad ↘", "diag:dpad:dr"),
            (vec!["ly.min", "lx.max"], "LS ↘", "diag:ls:dr"),
            (vec!["ry.max", "rx.min"], "RS ↖", "diag:rs:ul"),
            // Written the other way round — a hold is a SET.
            (vec!["lx.max", "ly.min"], "LS ↘", "diag:ls:dr"),
            // A hand-written partial deflection: still the diagonal.
            (vec!["ly.-16384", "lx.max"], "LS ↘", "diag:ls:dr"),
            // The hat+stick double-binding EVERY in-box template writes: one
            // presented diagonal naming both mechanisms — joined by "and", not
            // by `+`. `+` on this row means ANOTHER CONTROL ("D-pad ↘ + A"), so
            // "D-pad + LS ↘" read as a control called "D-pad" and a control
            // called "LS ↘": the mechanism holding half the diagonal lost its
            // direction, and one control looked like two. See `held_label`.
            (
                vec!["dpad.down", "dpad.right", "ly.min", "lx.max"],
                "D-pad and LS ↘",
                "diag:dpad:dr",
            ),
        ];
        for (hold, words, token) in cases {
            let mut payload = sample();
            payload.macros.macros[0].steps = vec![step(&hold.to_vec(), Some(50), None, false)];
            let out = render_map(&page(), &payload, None);
            let html = &out.html;
            assert!(
                html.contains(&format!(">{words}<")),
                "{hold:?} must read as {words}: {html}"
            );
            assert!(
                html.contains(&format!(r#"data-cell="0|{token}""#)),
                "{hold:?}: {html}"
            );
            // The pick is LIT, and the ledger names the file's own spelling.
            let cell = html
                .find(&format!(r#"data-cell="0|{token}""#))
                .expect("the diagonal cell");
            let before = &html[..cell];
            assert!(
                before
                    .rsplit("<button")
                    .next()
                    .unwrap()
                    .contains("maccell on"),
                "{hold:?}: the diagonal cell is not lit: {html}"
            );
            assert!(
                html.contains(&format!("↘ = {}", hold.join(" + ")))
                    || html.contains(&format!("↖ = {}", hold.join(" + "))),
                "{hold:?}: the ledger does not spell the stored pair: {html}"
            );
        }

        // A hand-written PARTIAL deflection is labelled, never rewritten.
        let mut payload = sample();
        payload.macros.macros[0].steps =
            vec![step(&["ly.-16384", "lx.max"], Some(50), None, false)];
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains("maccell on approx"), "{}", out.html);
        assert!(
            out.html.contains("as written, not at full deflection"),
            "{}",
            out.html
        );
        // …and the TOML it offers back is still exactly what the file said.
        assert!(out.html.contains("ly.-16384"), "{}", out.html);
    }

    /// The named edge case that keeps the editor honest: `down + forward + up`
    /// is CONTRADICTORY and must never fold. Which diagonal would it be — and
    /// what the pad publishes depends on the slot's `socd` policy, resolved at
    /// plan time, which this page cannot see. So it shows three cardinals.
    #[test]
    fn a_contradictory_step_stays_three_cardinals() {
        let mut payload = sample();
        payload.macros.macros[0].steps = vec![step(
            &["dpad.down", "dpad.right", "dpad.up"],
            Some(50),
            None,
            false,
        )];
        let out = render_map(&page(), &payload, None);
        let html = &out.html;
        assert!(
            html.contains(">D-pad ↓ + D-pad → + D-pad ↑<"),
            "no diagonal is guessed: {html}"
        );
        assert!(
            !html.contains("maccell on isdiag"),
            "a diagonal was fabricated: {html}"
        );
        assert!(
            !html.contains(r#"class="maccell part""#),
            "nothing is half of anything here: {html}"
        );
        assert_eq!(
            html.matches(r#"class="maccell on""#).count(),
            3,
            "three cardinals, lit as themselves: {html}"
        );
    }

    /// THE LEDGER SURVIVES A NARROW CARD.
    ///
    /// `.macexp` — the span that says `↘ = dpad.down + dpad.right`, and the
    /// only reason the fold is allowed to call two holds one control — is a
    /// flex child of a bar capped at 52% of the card. Driven on the real page
    /// it measured 154px at a 1440 viewport, 60px at 1100 and **0px at 820**,
    /// `display: block` the whole way down: the honesty line disappeared with
    /// no rule saying it should and nothing left to hover.
    ///
    /// So the pair is stated in the row's own `title` as well, which is the one
    /// thing on this row that cannot be truncated — and the narrow-width drop
    /// of the span (studio.css, `@media (max-width: 45rem)`) is now a decision
    /// instead of an accident.
    #[test]
    fn the_stored_pair_is_on_the_row_itself_not_only_in_the_ledger_span() {
        let mut payload = sample();
        payload.macros.macros[0].steps = vec![
            step(&["dpad.down", "dpad.right", "A"], Some(50), None, false),
            step(&["ly.-16384", "lx.max"], Some(50), None, false),
            step(&["B"], Some(50), None, false),
        ];
        let out = render_map(&page(), &payload, None);
        let html = &out.html;
        // The row title carries what it holds AND what the file stores.
        assert!(
            html.contains(
                "step 1 holds D-pad ↘ + A for 50 ms (the engine runs it for 50 ms) \
                 — ↘ = dpad.down + dpad.right"
            ),
            "the row title must carry the stored pair: {html}"
        );
        // …spelled as the FILE spells it, not as the canonical pair.
        assert!(
            html.contains("— ↘ = ly.-16384 + lx.max"),
            "the title must quote the file, not rewrite it: {html}"
        );
        // A row with no diagonal gains nothing — no dangling em dash.
        assert!(
            html.contains("step 3 holds B for 50 ms (the engine runs it for 50 ms)\"")
                || html.contains("step 3 holds B for 50 ms (the engine runs it for 50 ms)'"),
            "a plain row's title is unchanged: {html}"
        );
    }

    /// The ring is STATED, under the grid, on the page a reader with no
    /// JavaScript gets — they cannot tick a cell, but they can read what the
    /// columns mean and hand-write the pair into the TOML block below.
    #[test]
    fn the_ring_and_what_a_pick_writes_are_stated_on_the_page() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        assert!(html.contains(r#"class="macring""#), "{html}");
        assert!(
            html.contains("↑ ↖ ← ↙ ↓ ↘ → ↗ (numpad 8 7 4 1 2 3 6 9)"),
            "the ring, with the lookup digits: {html}"
        );
        assert!(
            html.contains("stores dpad.down + dpad.right on that step"),
            "what a pick WRITES, in the file's own words: {html}"
        );
        // The digits are a LOOKUP KEY, not a label: no numpad digit reaches the
        // glyph row. Every direction header is one arrow and nothing else — a
        // second line of digits under only 24 of 37 columns would make the
        // header ragged and re-introduce exactly the ornament the coloured
        // discs were stripped for. (`L3`/`R3` are the controls' own names.)
        for column in macro_columns("xbox360") {
            if Mechanism::of(&column.token).is_none() && !column.token.starts_with("diag:") {
                continue;
            }
            assert_eq!(
                column.glyph.chars().count(),
                1,
                "a direction header is one arrow: {}",
                column.glyph
            );
            assert!(
                "↑↓←→↖↗↙↘".contains(&column.glyph),
                "not an arrow: {}",
                column.glyph
            );
        }
        // …but every diagonal tooltip carries it, beside the move-list form —
        // and the move-list form is LABELLED AS FACING-RELATIVE. ksx has no
        // notion of facing (it publishes a direction, not a side of the screen)
        // and already ships the mirrored spelling of every motion because
        // player 2 is not an edge case. A bare "(d/f)" beside "down-right" read
        // as a second name for the same fact; for the player standing on the
        // right it is d/b, so it was a second name for the fact half the time.
        assert!(
            html.contains("down-right · numpad 3 · d/f in a move list, facing right"),
            "the tooltip is the lookup, and says which way it assumes you face: {html}"
        );
        // …and the ring line says the same thing once, for the whole card.
        assert!(
            html.contains("only down-FORWARD while you face right")
                && html.contains("ksx has no idea which way you are facing"),
            "the ring line owns the facing caveat: {html}"
        );
        // THE OFF-SCREEN GROUP. 37 columns do not fit: measured on the real
        // page the matrix is 551px of an 889px grid at a 1440 viewport, so
        // D-PAD and RIGHT STICK are both past the edge — and the D-pad is what
        // the motion buttons write by default. A user who sees only LEFT
        // STICK's ring ticks LEFT STICK's ↘ on a preset whose game reads the
        // hat, which is `Issue::MacroHoldsOtherMechanism` arriving after a
        // save. The page has to say the other groups are there.
        assert!(
            html.contains("THERE ARE THREE OF THESE GROUPS")
                && html.contains("the grid scrolls sideways to reach them"),
            "the ring line says the groups off the edge exist: {html}"
        );
    }

    /// The three policy selects offer exactly what ksx-core accepts, spelled
    /// the way a config file stores it. A word this page invents is a select
    /// that writes a preset the loader refuses.
    #[test]
    fn the_policy_vocabularies_match_ksx_core() {
        use ksx_core::macros::{Interrupt, OnRelease, Retrigger};
        let out = render_map(&page(), &sample(), None);
        for word in OnRelease::ALL.iter().map(|p| p.as_str()) {
            assert!(
                out.html.contains(&format!("<option>{word}</option>")),
                "{word}"
            );
        }
        for word in Retrigger::ALL.iter().map(|p| p.as_str()) {
            assert!(
                out.html.contains(&format!("<option>{word}</option>")),
                "{word}"
            );
        }
        for word in Interrupt::ALL.iter().map(|p| p.as_str()) {
            assert!(
                out.html.contains(&format!("<option>{word}</option>")),
                "{word}"
            );
        }
        // Defaults FIRST in each list: the select is a draft control, and an
        // SSR paint cannot mark an option selected, so the resting value has
        // to be the one a macro has when it says nothing.
        assert_eq!(OnRelease::ALL[0].as_str(), "finish");
        assert_eq!(Retrigger::ALL[0].as_str(), "ignore");
        assert_eq!(Interrupt::ALL[0].as_str(), "none");
    }

    /// THE SHAPE (docs/INPUT-TRANSFORMS.md §6.2): rows are steps, columns are
    /// the slot's controls, a cell is held or not. You SEE the sequence.
    #[test]
    fn the_piano_roll_puts_steps_on_rows_and_controls_on_columns() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;

        // 4 steps × 37 columns, every cell addressable as `step|token`.
        assert_eq!(
            html.matches(r#"class="maccell"#).count(),
            4 * MACRO_COLUMN_COUNT,
            "one cell per (step, column): {html}"
        );
        assert!(html.contains(r#"data-cell="0|dpad.down""#), "{html}");
        assert!(html.contains(r#"data-cell="3|A""#), "{html}");
        // THE POINT: step 2 holds `dpad.down` + `dpad.right`, and the grid
        // lights the D-pad ↘ CELL — one pick, not two stray ticks.
        assert!(html.contains(r#"data-cell="1|diag:dpad:dr""#), "{html}");
        assert_eq!(
            html.matches(r#"class="maccell on isdiag""#).count(),
            1,
            "exactly one diagonal is lit in this hadouken: {html}"
        );
        assert!(
            html.contains("step 2 holds D-pad ↘ (down-right)"),
            "the cell says what it means in the pad's own words: {html}"
        );
        // …and the two cardinals it is made of are still shown, subordinate,
        // because they really are in the file.
        assert_eq!(
            html.matches(r#"class="maccell part""#).count(),
            2,
            "↓ and → are half of ↘, and say so: {html}"
        );
        assert!(
            html.contains("as half of ↘ — the ↘ column beside it is the pick"),
            "{html}"
        );
        // The plain single-control holds: ↓, →, A.
        assert_eq!(
            html.matches(r#"class="maccell on""#).count(),
            3,
            "↓, →, A = three plainly held cells: {html}"
        );
        assert!(html.contains("step 1 does not hold"), "{html}");
        // Rows: numbered, in order, wearing their duration — including the one
        // authored in frames, which must still READ as frames.
        assert_eq!(
            html.matches(r#"class="macnum""#).count(),
            4,
            "one numbered row per step: {html}"
        );
        assert_eq!(text_in(html, "macnum").as_deref(), Some("1"), "{html}");
        assert_eq!(text_in(html, "macdur").as_deref(), Some("50 ms"), "{html}");
        assert!(
            html.contains("3 fr · 50 ms"),
            "the authored unit survives: {html}"
        );
        // Columns: the persona's identity GLYPHS, but UNIFORM — no `id-*`
        // accent class reaches a grid header, so the coloured discs the art and
        // the legend wear cannot follow the glyph in here (a header row of them
        // at column width is noise, not information).
        assert!(html.contains(r#"class="maccolid""#), "{html}");
        assert!(
            !html.contains("maccolid id-"),
            "no accent in a header: {html}"
        );
        assert!(html.contains("D-pad ↓ (dpad.down)"), "{html}");
        // The head line and the policies, in words.
        assert_eq!(
            text_in(html, "machead").as_deref(),
            Some("hadouken — 4 steps · 200 ms total"),
            "{html}"
        );
        assert!(
            html.contains(
                "on release: finish · retrigger: ignore · interrupt: none · repeat: once"
            ),
            "{html}"
        );
        // Step verbs, one payload per row.
        for act in ["up|1", "down|1", "insa|1", "insb|1", "del|1", "sel|1"] {
            assert!(
                html.contains(&format!(r#"data-macact="{act}""#)),
                "{act}: {html}"
            );
        }
        // The ends of the list cannot move further out.
        assert!(html.contains(r#"class="macbtn off""#), "{html}");
    }

    /// The sampling rule, VISIBLE (§0.2). A step below ~2 poll intervals is
    /// flagged inline with the reason — never silently accepted, and never
    /// silently rewritten either: the flag says which of the two happened.
    #[test]
    fn a_step_shorter_than_the_sampling_floor_is_flagged_with_the_reason() {
        let mut payload = sample();
        payload.macros.macros[0].steps[0] = step(&["dpad.down"], Some(5), None, false);
        payload.macros.macros[0].steps[1] = step(&["A"], Some(5), None, true);
        let out = render_map(&page(), &payload, None);
        let html = &out.html;
        assert!(html.contains("macrow short"), "the amber row class: {html}");
        assert!(
            html.contains(
                "5 ms is shorter than ~2 poll intervals (33 ms) — the game may never \
                           see it, so ksx raises this step to 33 ms"
            ),
            "{html}"
        );
        assert!(
            html.contains("allow_short is on, so it runs as written and the game may never see it"),
            "the opt-out is flagged too, differently: {html}"
        );
        // A step at or above the floor says nothing at all.
        assert!(html.contains(r#"class="macwarn off""#), "{html}");

        // The inline flag is short enough to fit beside the duration; the rule
        // it stands for is stated once, in full, in the card.
        assert!(html.contains(">5 ms — raised to 33 ms<"), "{html}");
        assert!(html.contains(">5 ms — may be missed<"), "{html}");
        assert!(
            html.contains("ksx raises a short step to 33 ms"),
            "the rule, stated once: {html}"
        );

        // Both units, or neither, is a fault the editor NAMES rather than
        // resolving — `MacroStepFile::duration` refuses both.
        assert_eq!(
            step_warning(&step(&[], Some(50), Some(3), false)),
            "two units"
        );
        assert_eq!(
            step_warning_long(&step(&[], Some(50), Some(3), false)),
            "says both ms and frames — exactly one, or the file is refused"
        );
        assert!(step_warning_long(&step(&[], None, None, false)).contains("no duration"));
    }

    /// The whole output of a read-only editor: the block you paste. It has to
    /// PARSE — so this feeds it to the real loader and checks the macro that
    /// comes back is the one the grid drew, trigger row included.
    #[test]
    fn the_generated_toml_block_parses_back_into_the_same_macro() {
        let mac = MacroView {
            on_release: "abort".to_owned(),
            interrupt: "opposing".to_owned(),
            steps: vec![
                step(&["dpad.down"], Some(50), None, false),
                step(&[], Some(20), None, true),
                step(&["dpad.right"], None, Some(3), false),
            ],
            triggers: vec!["P".to_owned(), "Q".to_owned()],
            ..hadouken()
        };
        let block = macro_toml(&mac);
        let file: ksx_config::PresetFile =
            toml::from_str(&format!("name = \"round trip\"\n{block}")).unwrap_or_else(|e| {
                panic!("the block we tell users to paste must parse: {e}\n{block}")
            });
        let core = file.to_core().expect("…and must load");
        assert_eq!(core.macros.defs.len(), 1);
        let loaded = &core.macros.defs[0];
        assert_eq!(loaded.name, "hadouken");
        assert_eq!(loaded.on_release, ksx_core::macros::OnRelease::Abort);
        assert_eq!(loaded.retrigger, ksx_core::macros::Retrigger::Ignore);
        assert_eq!(loaded.interrupt, ksx_core::macros::Interrupt::Opposing);
        assert_eq!(loaded.steps.len(), 3);
        // The unit survives the round trip: frames stay frames (§1c).
        assert_eq!(
            loaded.steps[2].duration,
            ksx_core::macros::StepDuration::Frames(3)
        );
        assert!(loaded.steps[1].allow_short && loaded.steps[1].hold.is_empty());
        // Two triggers, because many keys → one macro is ordinary multi-bind.
        assert_eq!(core.macros.triggers.len(), 2);

        // A macro with default policies emits none of them — a file that says
        // nothing must keep saying nothing.
        let plain = macro_toml(&hadouken());
        assert!(!plain.contains("on_release"), "{plain}");
        assert!(!plain.contains("retrigger"), "{plain}");
        assert!(!plain.contains("interrupt"), "{plain}");

        // No trigger yet: the row is COMMENTED, never a placeholder key that
        // would refuse to load.
        let untriggered = MacroView {
            triggers: Vec::new(),
            ..hadouken()
        };
        let block = macro_toml(&untriggered);
        assert!(block.contains("# macro.hadouken = \"<KEY>\""), "{block}");
        let file: ksx_config::PresetFile =
            toml::from_str(&format!("name = \"p\"\n{block}")).expect("still parses");
        assert!(file.to_core().expect("loads").macros.triggers.is_empty());
    }

    /// The trigger is the ONE macro edit that is a real write, and it goes
    /// through the binding path that already exists: `macro.<name>` is a
    /// function name the `map` verb takes (mapping.rs `apply_macro_trigger`).
    /// So the page offers it as a learnable control AND as a no-JS form —
    /// Bind and Clear only, because add/remove-one would have to read a key
    /// list the mapper payload's `bindings` map does not carry for macros.
    #[test]
    fn the_trigger_is_bound_through_the_existing_map_verb() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        assert!(html.contains(r#"data-fn="macro.hadouken""#), "{html}");
        assert!(html.contains("started by P"), "{html}");
        assert!(
            html.contains(r#"<input type="hidden" name="function" value="macro.hadouken">"#),
            "the no-JS form names the macro function: {html}"
        );
        assert!(
            html.contains(r#"class="macbind nojs" method="post" action="/map/bind""#),
            "{html}"
        );
        assert!(html.contains(r#"formaction="/map/clear""#), "{html}");
        assert!(
            html.contains(r#"ksx map --preset "IPAC P1" --function macro.hadouken --key P"#),
            "the exact CLI line, with the key it already has: {html}"
        );
        // …and a macro with no trigger prints the template, not a lie.
        let mut payload = sample();
        payload.macros.macros[0].triggers.clear();
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains("no trigger key yet"), "{}", out.html);
        assert!(
            out.html
                .contains("--function macro.hadouken --key &lt;KEY&gt;")
                || out.html.contains("--function macro.hadouken --key <KEY>"),
            "{}",
            out.html
        );
    }

    /// The card says what it DOES, and the three states are told apart: the
    /// provider could not read macros / the preset has none / here is one and
    /// here is what Save writes. "No macros" and "nobody told us" look
    /// identical on screen unless the page says which, and only one of them is
    /// the user's fault.
    #[test]
    fn the_editor_says_what_it_cannot_do_and_why() {
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html.contains("are a DRAFT until you press Save macro"),
            "the save model is stated where the grid is: {}",
            out.html
        );
        assert!(
            out.html
                .contains("New, Rename and Delete write immediately"),
            "…and so is the half that does NOT wait for Save: {}",
            out.html
        );
        // The dead read-only copy is GONE — it described a card that could not
        // save, and repeating it beside a Save button is the worst of both.
        assert!(
            !out.html.contains("nothing in this grid is saved"),
            "{}",
            out.html
        );
        assert!(
            !out.html.to_lowercase().contains("paste the toml block"),
            "copy-and-paste is no longer the primary path: {}",
            out.html
        );

        let mut empty = sample();
        empty.macros = crate::snapshot::MacroSnapshot::read("IPAC P1", Vec::new());
        let out = render_map(&page(), &empty, None);
        assert!(
            out.html.contains("This preset has no macros yet"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("press ＋ New macro"),
            "the way out of an empty preset is named: {}",
            out.html
        );
        // …and NOTHING is drawn in the grid. v11 painted a browser-only
        // "my-macro" here, which is what produced `preset "IPAC P1" defines no
        // macro called "my-macro"` when its trigger was bound.
        assert!(!out.html.contains("my-macro"), "{}", out.html);
        assert!(!out.html.contains("data-cell="), "{}", out.html);
        assert!(
            out.html.contains(r#"class="macgrid empty""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"class="mactrigger off""#),
            "a trigger with nothing to start renders inert: {}",
            out.html
        );

        let mut blind = sample();
        blind.macros = crate::snapshot::MacroSnapshot::unavailable("this daemon predates macros");
        let out = render_map(&page(), &blind, None);
        assert!(out.html.contains("could not be read"), "{}", out.html);
        assert!(
            out.html.contains("this daemon predates macros"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"class="card macrocard off""#),
            "{}",
            out.html
        );
    }

    /// v12: the card WRITES. Every verb is on screen, pointed at the one save
    /// seam, and the name on it is always a name the preset holds.
    #[test]
    fn the_macro_card_offers_the_four_writes_and_invents_no_macro() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        for act in ["macro-save", "macro-new", "macro-rename", "macro-delete"] {
            assert!(
                html.contains(&format!(r#"data-act="{act}""#)),
                "{act} is not on the card: {html}"
            );
        }
        // The create affordance is a NAME plus a button — not a button that
        // mints a draft nobody asked for.
        assert!(html.contains(r#"class="macnewin""#), "{html}");
        assert!(html.contains("＋ New macro"), "{html}");
        // The name the card shows is the preset's own.
        assert!(html.contains(r#"data-fn="macro.hadouken""#), "{html}");
        assert!(!html.contains("my-macro"), "{html}");
        // The save state is visible without hunting (Victor: "where is save?").
        assert!(
            html.contains(r#"class="btn btn-mini macsave off""#),
            "{html}"
        );
    }

    /// The user's own question, answered on screen: "a 60fps frame is only
    /// like sixteenth milliseconds? maybe we can show that math." The floor
    /// rides along in the SAME units, which is what makes an amber row
    /// self-explanatory.
    #[test]
    fn the_duration_editor_shows_the_frame_math_and_the_floor() {
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html
                .contains("The engine can only see steps of 33 ms (2.0 frames @ 60 Hz)"),
            "the floor, in both units: {}",
            out.html
        );

        // Frames → ms, at 60 Hz, exactly as the engine rounds it.
        let three = step(&[], None, Some(3), false);
        assert!(
            frame_math(Some(&three), 60.0).starts_with("3 frames @ 60 Hz = 50.0 ms"),
            "{}",
            frame_math(Some(&three), 60.0)
        );
        assert!(frame_math(Some(&step(&[], None, Some(1), false)), 60.0).starts_with("1 frame @"));
        // ms → frames, the other direction.
        assert!(
            frame_math(Some(&step(&[], Some(50), None, false)), 60.0)
                .starts_with("50 ms = 3.0 frames @ 60 Hz"),
            "{}",
            frame_math(Some(&step(&[], Some(50), None, false)), 60.0)
        );
        // A non-60 target rate is a DISPLAY convenience, and the line says so
        // rather than pretending the file can store one: ksx counts frames at
        // 60 Hz, so it names the ms value that matches the game instead.
        let at57 = frame_math(Some(&three), 57.0);
        assert!(at57.contains("3 frames @ 57 Hz = 52.6 ms"), "{at57}");
        assert!(at57.contains("ksx counts frames at 60 Hz"), "{at57}");
        assert!(at57.contains("this step runs 50.0 ms"), "{at57}");
        assert!(at57.contains("enter 53"), "{at57}");
        // A fractional rate keeps its two decimals — 59.94 is not "60".
        assert_eq!(rate_text(59.94), "59.94");
        assert_eq!(rate_text(60.0), "60");
        assert!(
            frame_math(Some(&three), 59.94).contains("@ 59.94 Hz"),
            "{}",
            frame_math(Some(&three), 59.94)
        );
        // The faults keep saying what they are here too.
        assert!(frame_math(Some(&step(&[], Some(5), Some(1), false)), 60.0)
            .starts_with("This step says both ms and frames"));
        assert!(frame_math(Some(&step(&[], None, None, false)), 60.0)
            .starts_with("This step has no duration"));
    }

    /// FIX 1 — EVERY ROW SAYS WHAT IT HOLDS, in the pad's own words, without
    /// being hovered, clicked or decoded.
    ///
    /// The bug this exists for was never in the engine: Victor's "hadouken"
    /// had rows ↓ then → then X and no diagonal at all, because a diagonal is
    /// not an input you bind — it IS down+forward held together, i.e. ONE ROW
    /// HOLDING TWO CONTROLS. Two lit cells twelve columns apart do not say
    /// that. This line does, on every row, and the row that holds more than one
    /// control is the one that gets the accent.
    #[test]
    fn every_step_says_what_it_holds_in_words() {
        let mut payload = sample();
        // A neutral gap, appended: the empty row is a step too, and reads as
        // one rather than as a row somebody forgot to fill in.
        payload.macros.macros[0]
            .steps
            .push(step(&[], Some(50), None, false));
        // …and the single most common macro step in existence: the diagonal
        // WITH the attack that ends the motion.
        payload.macros.macros[0].steps.push(step(
            &["dpad.down", "dpad.right", "A"],
            Some(50),
            None,
            false,
        ));
        let out = render_map(&page(), &payload, None);
        let html = &out.html;

        // The hadouken steps, in words. Step 2 is the diagonal — and it reads
        // as ONE control, because that is what a player means by it.
        assert!(html.contains(">D-pad ↓<"), "step 1's readout: {html}");
        assert!(
            html.contains(">D-pad ↘<"),
            "the diagonal, named the way a player names it: {html}"
        );
        assert!(html.contains(">(nothing — neutral gap)<"), "{html}");
        // `down + forward + A` folds and keeps the button — exact-set matching
        // would have failed this, and it settles the whole recognition rule.
        assert!(html.contains(">D-pad ↘ + A<"), "{html}");

        // THE LEDGER: what the file actually holds, beside the words. The lens
        // is only honest if the storage is visible without opening the TOML.
        assert_eq!(
            html.matches(">↘ = dpad.down + dpad.right<").count(),
            2,
            "both diagonal rows spell out the pair they store: {html}"
        );
        assert_eq!(hold_expand(&[]), "");
        assert_eq!(hold_expand_cls(&["A".to_owned()]), "macexp off");

        // The class carries the distinction the words make, so CSS can accent
        // the row that really holds several things — counted over PRESENTED
        // controls, so a diagonal alone is no longer "two".
        assert_eq!(
            html.matches(r#"class="machold both""#).count(),
            1,
            "only `↘ + A` holds more than one presented control: {html}"
        );
        assert_eq!(
            html.matches(r#"class="machold none""#).count(),
            1,
            "and exactly one holds nothing: {html}"
        );
        assert_eq!(hold_cls(&[]), "machold none");
        assert_eq!(hold_cls(&["A".to_owned()]), "machold");
        assert_eq!(
            hold_cls(&["dpad.down".to_owned(), "dpad.right".to_owned()]),
            "machold",
            "a diagonal is ONE presented control"
        );
        assert_eq!(
            hold_cls(&[
                "dpad.down".to_owned(),
                "dpad.right".to_owned(),
                "A".to_owned()
            ]),
            "machold both"
        );
    }

    /// FIX 1 — the concept itself, stated once, permanently, and NOT behind a
    /// disclosure: it is the fact the piano roll cannot draw, and the evening
    /// it cost was spent looking for a timing bug that was never there.
    #[test]
    fn the_chord_rule_is_stated_where_it_cannot_be_missed() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        let at = html
            .find(r#"class="macconcept""#)
            .unwrap_or_else(|| panic!("the concept line is not on the page: {html}"));
        assert!(
            html.contains("a diagonal ")
                && html.contains("is ONE step holding ↓ and →, not two steps."),
            "the sentence itself: {html}"
        );
        // …and the answer to it, in the same breath: you PICK the diagonal, and
        // ksx writes the pair. That is Victor's insight, stated where the
        // mistake used to be made.
        assert!(
            html.contains("So pick the diagonal:")
                && html.contains("ticking ↘ stores ")
                && html.contains("dpad.down + dpad.right"),
            "the lens is not explained where it is used: {html}"
        );
        // Before the only nested disclosure this card has, so nothing has to be
        // opened to read it. (The card is a <details>; this is about not
        // burying the rule INSIDE another one.)
        let toml = html
            .find(r#"<details class="mactomlbox">"#)
            .unwrap_or_else(|| panic!("the TOML block moved: {html}"));
        assert!(at < toml, "the rule is buried under a disclosure: {html}");
        // And the intro no longer says "a quarter-circle is three rows" without
        // saying which of them holds two things.
        assert!(
            html.contains("then ↘ (which ") && html.contains("is ↓ and → together on ONE row)"),
            "the intro spells the middle step out: {html}"
        );
    }

    /// FIX 1c — the motion helpers generate on the mechanism THIS PRESET's own
    /// direction keys drive. A pad has three ways to say "right" and ksx
    /// publishes exactly what a step names, so a quarter-circle written in dpad
    /// holds on a stick preset is published faithfully and read by nobody —
    /// `Issue::MacroHoldsOtherMechanism`, which only arrives after a save.
    #[test]
    fn the_motion_helpers_name_the_mechanism_this_preset_drives() {
        // The sample slot binds `lx.min` — a left-stick direction.
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html.contains("These write the left stick (lx/ly)"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("read by nobody: that is the trap"),
            "{}",
            out.html
        );

        // A dpad preset gets dpad motions.
        let mut payload = sample();
        payload.mapper.slots[0].bindings.remove("lx.min");
        payload.mapper.slots[0]
            .bindings
            .insert("dpad.left".to_owned(), vec!["M".to_owned()]);
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains("These write the dpad"), "{}", out.html);

        // A preset driving BOTH says so rather than picking silently.
        let mut payload = sample();
        payload.mapper.slots[0]
            .bindings
            .insert("dpad.left".to_owned(), vec!["N".to_owned()]);
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html
                .contains("direction keys drive the left stick (lx/ly) and the dpad")
                || out
                    .html
                    .contains("direction keys drive the dpad and the left stick (lx/ly)"),
            "{}",
            out.html
        );

        // An INERT row is a function the preset lists, not a direction the
        // player can produce — same rule as ksx-config's `driven_mechanisms`.
        let mut payload = sample();
        payload.mapper.slots[0]
            .bindings
            .insert("lx.min".to_owned(), vec!["None".to_owned()]);
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html
                .contains("this preset binds no direction keys of its own"),
            "{}",
            out.html
        );
    }

    /// FIX 2 — a step authored in FRAMES is warned in frames. The old text told
    /// a `frames = 1` author "16 ms — raised to 33 ms": true, about a number
    /// they never typed, and it hands them the conversion instead of the
    /// answer.
    #[test]
    fn a_frame_authored_step_is_told_the_floor_in_frames() {
        assert_eq!(
            frames_ms(MIN_STEP_FRAMES),
            MIN_STEP_MS,
            "the two floors are one floor"
        );

        let one = step(&["A"], None, Some(1), false);
        assert_eq!(step_warning(&one), "1 fr — raised to 2 fr");
        assert!(
            step_warning_long(&one).starts_with("1 frame is shorter than the 2-frame floor (33 ms"),
            "{}",
            step_warning_long(&one)
        );
        assert!(
            step_warning_long(&one).contains("raises this step to 2 frames (33 ms)"),
            "{}",
            step_warning_long(&one)
        );
        let opted = step(&["A"], None, Some(1), true);
        assert_eq!(step_warning(&opted), "1 fr — may be missed");
        assert!(step_warning_long(&opted).contains("allow_short is on"));
        // An ms author still hears milliseconds — the unit is the AUTHOR's.
        assert_eq!(
            step_warning(&step(&["A"], Some(5), None, false)),
            "5 ms — raised to 33 ms"
        );
        // Two frames is the floor, so it says nothing at all.
        assert_eq!(step_warning(&step(&["A"], None, Some(2), false)), "");

        // And it reaches the page.
        let mut payload = sample();
        payload.macros.macros[0].steps[0] = one;
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains(">1 fr — raised to 2 fr<"), "{}", out.html);
        assert!(out.html.contains("macrow short"), "{}", out.html);
        // The rule under the grid carries both units now.
        assert!(
            out.html
                .contains("33 ms, or 2 frames if you are counting frames"),
            "{}",
            out.html
        );
    }

    /// FIX 2 — the save-confirmation bar and the duration field's own warn
    /// class exist in the SSR document and ship inert. Both are client state
    /// (every write on this card is a fetch, and an SSR paint has selected no
    /// step and asked no question), but the ELEMENTS must be there: the client
    /// only ever changes a class string, never inserts a panel (ledger #13/#14
    /// — a show inserted mid-document shifts every show after it).
    #[test]
    fn the_short_step_confirmation_ships_present_and_inert() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        assert!(html.contains(r#"class="macconfirm off""#), "{html}");
        assert!(html.contains(r#"data-act="macro-save-anyway""#), "{html}");
        assert!(html.contains(r#"data-act="macro-save-cancel""#), "{html}");
        // Empty, but PRESENT: an empty text slot renders the zero-width
        // placeholder that keeps the node adoptable, which is exactly what the
        // client needs in order to write the question into it later.
        assert_eq!(
            text_in(html, "macconfirmline").as_deref(),
            Some("\u{200b}"),
            "{html}"
        );
        // FIX 2: the duration box is per ROW now, and it is a plain box until
        // the number in it is below the floor.
        assert!(html.contains(r#"class="macrowdur""#), "{html}");
        assert!(
            !html.contains("macrowdur short"),
            "nothing in the sample is short: {html}"
        );
        // The six motion buttons, each carrying its own payload.
        for m in ["qcf", "qcb", "hcf", "hcb", "dpf", "dpb"] {
            assert!(
                html.contains(&format!(r#"data-macmotion="{m}""#)),
                "{m}: {html}"
            );
        }
        // FIX 3 — THE LABELS SPEAK DIAGONALS. They used to spell the pair a
        // diagonal is stored as ("¼ → · ↓ · ↓+→ · →"), which was the right
        // teaching move before diagonals were first-class and contradicts the
        // abstraction now: the grid has a ↘ column, the row readout calls it
        // one control, and only the buttons still called it two. The pair is
        // taught in exactly ONE place — the row's expansion ledger, asserted
        // just below.
        assert!(html.contains("¼ → · ↓ ↘ →"), "{html}");
        assert!(html.contains("DP → · → ↓ ↘"), "{html}");
        assert!(
            !html.contains("↓+→") && !html.contains("↓ + →"),
            "a motion label still spells a diagonal as its pair: {html}"
        );
    }

    /// FIX 1 — the editor cannot construct the zero-step macro the writer
    /// refuses (`mapping::save_macro`: empty steps is a refusal, not a delete).
    /// The ✕ on the LAST remaining step empties it instead of removing it, and
    /// says so; the "＋ Add step" toolbar is above the grid and belongs to no
    /// row, so growing a macro never depends on one already being there.
    #[test]
    fn the_editor_cannot_reach_a_zero_step_macro() {
        // A macro with several steps: every ✕ is an ordinary delete.
        let out = render_map(&page(), &sample(), None);
        assert!(
            out.html.contains(r#"title="delete this step""#),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains("a macro needs at least one"),
            "a multi-step macro must not offer the clear wording: {}",
            out.html
        );

        // …and with exactly one step left, the same button says what it will
        // really do. A delete that silently became a clear would read as a
        // broken delete, which is the other half of this fix.
        let mut payload = sample();
        payload.macros.macros[0].steps.truncate(1);
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html
                .contains("clear this step (a macro needs at least one"),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"title="delete this step""#),
            "the last step still claims it deletes: {}",
            out.html
        );

        // The add affordance that does not live on a row — SSR'd like every
        // other control here (hidden without JavaScript, because it edits a
        // draft), so the seam is pinned even though the click is client-only.
        assert!(out.html.contains(r#"class="macsteptools""#), "{}", out.html);
        assert!(out.html.contains("＋ Add step"), "{}", out.html);
    }

    /// FIX 2 — a duration is edited ON ITS ROW: every row carries its own
    /// number, its own `unit|N` toggle and its own index. There is no
    /// select-a-step-first mode left to lose to a poll.
    #[test]
    fn every_row_carries_its_own_duration_editor() {
        let mut payload = sample();
        payload.macros.macros[0].steps = vec![
            MacroStepView {
                hold: vec!["dpad.down".to_owned()],
                ms: Some(50),
                frames: None,
                allow_short: false,
            },
            MacroStepView {
                hold: vec!["A".to_owned()],
                ms: None,
                frames: Some(3),
                allow_short: false,
            },
        ];
        let out = render_map(&page(), &payload, None);
        let html = &out.html;
        for (i, (value, unit)) in [("50", "ms"), ("3", "fr")].iter().enumerate() {
            assert!(
                html.contains(&format!(r#"data-durrow="{i}""#)),
                "row {i} has no duration box: {html}"
            );
            assert!(
                html.contains(&format!(r#"data-macact="unit|{i}""#)),
                "row {i} has no unit toggle: {html}"
            );
            assert!(
                html.contains(&format!(r#"value="{value}""#)),
                "row {i}: {html}"
            );
            assert!(
                html.contains(&format!(">{unit}<")),
                "row {i} does not read as {unit}: {html}"
            );
        }
        // The unit is the AUTHOR's, both ways round — a frames row says frames
        // and offers ms, never the other way about.
        assert!(html.contains("step 2 is authored in FRAMES"), "{html}");
        assert!(
            html.contains("step 1 is authored in MILLISECONDS"),
            "{html}"
        );
    }

    /// The copy-and-paste path is still there and is no longer the point: a
    /// collapsed detail, labelled as the sharing/hand-editing route.
    #[test]
    fn the_toml_block_is_secondary_now() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        assert!(html.contains(r#"<details class="mactomlbox">"#), "{html}");
        assert!(
            html.contains("Advanced — this macro as TOML"),
            "the summary says what it is for: {html}"
        );
        assert!(
            html.contains("You do not need this to keep your work"),
            "{html}"
        );
        // …and the block itself is still exactly the one the loader accepts
        // (`the_generated_toml_block_parses_back_into_the_same_macro`).
        assert!(html.contains("[macros.hadouken]"), "{html}");
    }

    /// "＋ New macro" writes a REAL table, so what it writes has to load. A
    /// starter body the daemon refuses would be a button that fails the first
    /// time anybody presses it.
    #[test]
    fn the_body_new_macro_writes_is_one_the_loader_accepts() {
        let body = new_macro_body("uppercut");
        assert_eq!(body.steps.len(), 1, "a macro with no steps is refused");
        let block = macro_toml(&body);
        let file: ksx_config::PresetFile =
            toml::from_str(&format!("name = \"p\"\n{block}")).expect("the new body must parse");
        let core = file.to_core().expect("…and must load");
        assert_eq!(core.macros.defs.len(), 1);
        assert_eq!(core.macros.defs[0].name, "uppercut");
        assert_eq!(core.macros.defs[0].steps.len(), 1);
        // It arrives with no trigger, and the block says so in a COMMENT
        // rather than a placeholder key that would not load.
        assert!(core.macros.triggers.is_empty());
    }

    /// A first-time reader has to learn two words from this card alone: what a
    /// macro is, and what a trigger is. Nobody should have to open
    /// docs/INPUT-TRANSFORMS.md to press a button.
    #[test]
    fn the_card_explains_a_macro_and_its_trigger_in_words() {
        let out = render_map(&page(), &sample(), None);
        let html = &out.html;
        assert!(html.contains("A MACRO is a timed sequence"), "{html}");
        assert!(
            html.contains("A TRIGGER is the panel key that STARTS the macro"),
            "{html}"
        );
        assert!(
            html.contains("Trigger — the key that STARTS this macro"),
            "the trigger row names its own job: {html}"
        );
        assert!(
            html.contains("A macro with no trigger is inert"),
            "…and what happens if you skip it: {html}"
        );
    }

    /// `?macro=NAME` picks the table, and the tabs are anchors — so a page
    /// with no JavaScript can walk every macro a preset defines, exactly like
    /// `?slot=N` walks its slots.
    #[test]
    fn the_macro_tabs_are_links_and_the_query_picks_one() {
        let mut payload = sample();
        payload.macros.macros.push(MacroView {
            name: "shoryuken".to_owned(),
            steps: vec![step(&["dpad.right"], Some(50), None, false)],
            ..hadouken()
        });
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html
                .contains(r#"href="/map?slot=1&amp;macro=shoryuken""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"class="mactab active""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("hadouken · 4 steps"), "{}", out.html);

        payload.macro_selected = "shoryuken".to_owned();
        let out = render_map(&page(), &payload, None);
        assert!(
            out.html.contains("shoryuken — 1 step · 50 ms total"),
            "{}",
            out.html
        );
        assert_eq!(
            out.html.matches(r#"class="maccell"#).count(),
            MACRO_COLUMN_COUNT,
            "one step = one row of cells: {}",
            out.html
        );
    }

    // ---- v13: AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3) ------------------

    /// The legend badge says what the game will SEE, not what was typed. A
    /// page that echoed an undeliverable number back would be lying about the
    /// file on the file's behalf.
    #[test]
    fn the_legend_badge_states_the_effective_rate() {
        let out = render_map(&page(), &sample(), None);
        // The fixture's B auto-fires at 12 Hz, which IS deliverable: 83 ms a
        // cycle splits into 42 pressed and 41 released, both over the floor.
        assert!(out.html.contains("turbo 12 Hz"), "{}", out.html);

        // 30 Hz is the fastest a file may ASK for and about 15 Hz is the
        // fastest anything can be given, because each half of a cycle has to
        // survive a 60 Hz poll. The badge says the second number.
        let mut payload = sample();
        for slot in &mut payload.mapper.slots {
            slot.turbo.insert("B".to_owned(), 30);
        }
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains("turbo ~15 Hz"), "{}", out.html);
        assert!(
            !out.html.contains("turbo 30 Hz"),
            "the asked-for rate must not be echoed as if it were real: {}",
            out.html
        );
    }

    /// A control with no rate renders an EMPTY badge (CSS collapses it), never
    /// a placeholder — and its title says where to set one.
    #[test]
    fn a_control_without_turbo_says_so_in_its_title() {
        let out = render_map(&page(), &sample(), None);
        assert!(out.html.contains("A does not auto-fire"), "{}", out.html);
    }

    /// The turbo arithmetic, pinned. These are the numbers
    /// `ksx_core::TurboBinding` produces; this crate depends on no other ksx
    /// crate, so they are repeated here and pinned rather than imported.
    #[test]
    fn the_turbo_arithmetic_is_the_engines() {
        assert_eq!((turbo_on_ms(12), turbo_off_ms(12)), (42, 41));
        assert_eq!(effective_turbo_hz(12), 12);
        // At the ceiling both halves land on the floor: 33 + 33 = 66 ms.
        assert_eq!((turbo_on_ms(30), turbo_off_ms(30)), (33, 33));
        assert_eq!(effective_turbo_hz(30), 15);
        // Above the ceiling is clamped, not refused, and lands in the same
        // place — which is why the badge for 240 Hz reads the same as for 30.
        assert_eq!(effective_turbo_hz(240), 15);
        // 15 Hz is the fastest rate that is BOTH askable and deliverable.
        assert_eq!(effective_turbo_hz(15), 15);
        // Zero means "off" in a file, never a division by zero here.
        assert_eq!(effective_turbo_hz(0), 1);
    }

    /// The macro card's own live math: the same "both numbers, always" promise
    /// the duration field makes, applied to `repeat = "turbo"`.
    #[test]
    fn the_macro_turbo_math_states_both_numbers() {
        let mut mac = hadouken(); // 200 ms of steps
        mac.repeat = "turbo".to_owned();
        mac.turbo_hz = Some(30);
        let line = turbo_math(Some(&mac));
        assert!(line.contains("Requested 30 Hz"), "{line}");
        // 30 Hz asks for a 33 ms cycle; the sequence alone is 200 ms, so the
        // gap falls to the floor and one cycle is 233 ms — about 4 Hz.
        assert!(line.contains("effective ~4 Hz"), "{line}");
        assert!(line.contains("200 ms long"), "{line}");
        assert!(line.contains("33 ms"), "the floor is named: {line}");

        // `once` and `while-held` explain themselves rather than showing a
        // rate they do not have.
        mac.repeat = "once".to_owned();
        assert!(turbo_math(Some(&mac)).contains("One run per press"), "once");
        mac.repeat = "while-held".to_owned();
        assert!(turbo_math(Some(&mac)).contains("NO gap"), "while-held");
    }

    /// The policy line and the pasteable TOML both carry the repeat setting —
    /// the readable half of the new selects, for a page with no JavaScript.
    #[test]
    fn the_repeat_policy_reaches_the_page_and_the_toml() {
        let mut mac = hadouken();
        mac.repeat = "turbo".to_owned();
        mac.turbo_hz = Some(10);
        assert!(
            macro_policy_line(Some(&mac)).contains("repeat: turbo (10 Hz)"),
            "{}",
            macro_policy_line(Some(&mac))
        );
        let toml = macro_toml(&mac);
        assert!(toml.contains("repeat = \"turbo\""), "{toml}");
        assert!(toml.contains("turbo_hz = 10"), "{toml}");
        assert!(!toml.contains("gap_ms"), "exactly one spelling: {toml}");

        // A `once` macro emits neither — a preset that predates repeat still
        // serializes to the block it always did.
        let plain = hadouken();
        let toml = macro_toml(&plain);
        assert!(!toml.contains("repeat"), "{toml}");
        assert!(!toml.contains("turbo_hz"), "{toml}");
    }

    /// The columns follow the PERSONA, like every other reader on this page:
    /// the same control is `A` on an Xbox slot and `✕` on a DualShock one.
    #[test]
    fn the_grid_columns_are_persona_aware() {
        let mut payload = sample();
        payload.selected = 3; // the PlayStation slot
        let out = render_map(&page(), &payload, None);
        // The GLYPH follows the persona — the header still NAMES the control the
        // way this pad does — but no `id-*` accent class comes with it.
        assert!(
            out.html.contains(r#"class="maccolid" title="✕ (A)""#),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"title="A (A)""#),
            "the Xbox spelling must not leak into a DS4 slot: {}",
            out.html
        );
        assert!(
            !out.html.contains("maccolid id-"),
            "a grid header carries no identity accent: {}",
            out.html
        );
        assert!(out.html.contains("step 4 holds ✕ (A)"), "{}", out.html);
    }

    /// Hostile bindings render escaped, not injected.
    #[test]
    fn hostile_binding_names_are_escaped() {
        let mut payload = sample();
        payload.mapper.slots[0]
            .bindings
            .insert("X".into(), vec!["<script>alert(1)</script>".into()]);
        let out = render_map(&page(), &payload, None);
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
    }
}
