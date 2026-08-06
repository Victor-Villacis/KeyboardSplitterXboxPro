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

use crate::render::{art_for, body_prefix, daemon_command, EmbeddedPage};
use crate::snapshot::{MapPayload, MapperSlot};

/// List slot names (binding-derived, compiler 0.2.0). The zones list appears
/// TWICE — once inside each stage show (xbox / ds4) — so the second
/// occurrence gets the `#2` suffix, exactly like the status page's
/// profileRows pair; both receive the same array.
const LIST_SLOT_TABS: &str = "list:slotTabs:array";
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

#[cfg(test)]
const ISLAND_COMPONENT: &str = "MapIsland";

/// The mapper's positional `show:createShow` seam (ledger #4), document order
/// in MapIsland.ts.
const SHOW_SLOT_NAME: &str = "show:createShow";
const MAP_SHOW_ORDER: [&str; 19] = [
    "header pill: running",
    "header pill: idle",
    "header pill: no daemon",
    // FIX 0: client-only — set by the page's own Pause action, so the user
    // cannot forget the cabinet is paused because they opened the mapper.
    "header pill: paused for mapping",
    // FIX 1: the unmissable banner, first child of <main>.
    "no-daemon banner (top of page)",
    // FIX 0: running-session banner + the one-click "Pause emulation & map".
    "banner: emulation running (pause & map)",
    // FIX 0: the road back — client-only, same flag as the pill.
    "banner: paused for mapping (resume)",
    "read-only banner + CLI fallback",
    "hint: click-a-control (learnable)",
    "stage: xbox art (+ zone layer)",
    "stage: ds4 art (+ zone layer)",
    // FIX 2: the third restore destination only exists when a backup does.
    "preset actions: restore-backup button",
    // v9: SERVER-driven now. A no-JS form POST 303s back to
    // /map?slot=N&flash=…, and these two are how that outcome reaches the
    // page (the client keeps reporting through the toast stack instead —
    // map.ts blanks these on adoption, so nothing is said twice).
    "saved flash: ok",
    "saved flash: error",
    "modal: overlay open",
    "modal: listening (countdown)",
    "modal: current binding + Clear",
    "modal: conflict (Replace / Cancel)",
    // v7 multi-select. APPENDED, deliberately: ledger #14 — a show inserted in
    // the middle shifts every show after it, so a new client-only overlay goes
    // LAST in document order (it is `position: fixed`, so the DOM position
    // costs nothing visually).
    "selection bar: N controls selected (multi-select)",
];
const MAP_SHOW_COUNT: usize = MAP_SHOW_ORDER.len();

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
    zone("ly.max", "▲", "dir", [24.0, 31.7, 7.0, 6.0], "chip"),
    zone("ly.min", "▼", "dir", [24.0, 47.7, 7.0, 6.0], "chip"),
    zone("lx.min", "◀", "dir", [17.25, 39.7, 5.5, 7.0], "chip"),
    zone("lx.max", "▶", "dir", [30.75, 39.7, 5.5, 7.0], "chip"),
    // Dpad cross.
    zone("dpad.up", "▲", "dir", [36.4, 50.6, 7.0, 9.0], "chip"),
    zone("dpad.down", "▼", "dir", [36.4, 69.2, 7.0, 9.0], "chip"),
    zone("dpad.left", "◀", "dir", [29.2, 59.9, 7.0, 9.0], "chip"),
    zone("dpad.right", "▶", "dir", [43.6, 59.9, 7.0, 9.0], "chip"),
    // Right stick: R3 hub + ring wedges.
    zone("rthumb", "R3", "hub", [62.5, 58.4, 8.0, 10.0], "round"),
    zone("ry.max", "▲", "dir", [62.5, 50.4, 7.0, 6.0], "chip"),
    zone("ry.min", "▼", "dir", [62.5, 66.4, 7.0, 6.0], "chip"),
    zone("rx.min", "◀", "dir", [55.75, 58.4, 5.5, 7.0], "chip"),
    zone("rx.max", "▶", "dir", [69.25, 58.4, 5.5, 7.0], "chip"),
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
    zone("ly.max", "▲", "dir", [33.8, 48.8, 7.0, 6.0], "chip"),
    zone("ly.min", "▼", "dir", [33.8, 64.8, 7.0, 6.0], "chip"),
    zone("lx.min", "◀", "dir", [27.05, 56.8, 5.5, 7.0], "chip"),
    zone("lx.max", "▶", "dir", [40.55, 56.8, 5.5, 7.0], "chip"),
    // Dpad arrows.
    zone("dpad.up", "▲", "dir", [18.5, 31.5, 5.4, 7.2], "chip"),
    zone("dpad.down", "▼", "dir", [18.5, 46.6, 5.4, 7.2], "chip"),
    zone("dpad.left", "◀", "dir", [12.9, 39.2, 5.4, 7.2], "chip"),
    zone("dpad.right", "▶", "dir", [23.9, 39.2, 5.4, 7.2], "chip"),
    // Right stick: R3 hub + ring wedges.
    zone("rthumb", "R3", "hub", [66.1, 56.8, 8.0, 10.0], "round"),
    zone("ry.max", "▲", "dir", [66.1, 48.8, 7.0, 6.0], "chip"),
    zone("ry.min", "▼", "dir", [66.1, 64.8, 7.0, 6.0], "chip"),
    zone("rx.min", "◀", "dir", [59.35, 56.8, 5.5, 7.0], "chip"),
    zone("rx.max", "▶", "dir", [72.85, 56.8, 5.5, 7.0], "chip"),
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
    SlotValue::Array(
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
                SlotValue::Object(vec![
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
    SlotValue::Array(
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
                ];
                fields.extend(key_chip_fields(z.fn_name, &keys, live));
                SlotValue::Object(fields)
            })
            .collect(),
    )
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
    SlotValue::Array(
        payload
            .mapper
            .slots
            .iter()
            .map(|s| {
                let active = selected.is_some_and(|sel| sel.number == s.number);
                SlotValue::Object(vec![
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

fn scalar_slots(
    payload: &MapPayload,
    selected: Option<&MapperSlot>,
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
    })
}

fn show_values(
    payload: &MapPayload,
    selected: Option<&MapperSlot>,
    flash: Option<&str>,
) -> [bool; MAP_SHOW_COUNT] {
    let art = selected.map(|s| art_for(&s.persona));
    let live = learnable(payload) && selected.is_some();
    let running = payload.session.reachable && payload.session.running;
    // Same rule as the status page's flash: an outcome that starts with
    // "error" is reported as one, everything else as a plain confirmation.
    let flash = flash.map(str::trim).filter(|f| !f.is_empty());
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    [
        running,
        payload.session.reachable && !payload.session.running,
        !payload.session.reachable,
        false, // "paused for mapping": client-only, set by the Pause action
        !payload.session.reachable, // the top-of-page no-daemon banner
        running, // the pause-and-map banner
        false, // the resume bar: client-only, same flag
        !live,
        live,
        art == Some(crate::render::ART_XBOX),
        art == Some(crate::render::ART_DS4),
        selected.is_some_and(|s| s.backup.is_some()),
        // v9: the no-JS flash, server-rendered from ?flash=.
        flash.is_some() && !flash_err,
        flash_err,
        false, // modal: client-only, never SSR-open
        false,
        false,
        false,
        false, // the multi-select bar: client-only, nothing is selected on SSR
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
    let scalars = scalar_slots(payload, selected, flash).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    let tabs = slot_tabs(payload, selected);
    let live = learnable(payload) && selected.is_some();
    let write = writable(payload) && selected.is_some();
    let zones = selected
        .map(|slot| zone_rows(slot, live))
        .unwrap_or(SlotValue::Array(Vec::new()));
    let legend = selected
        .map(|slot| legend_rows(slot, live, write))
        .unwrap_or(SlotValue::Array(Vec::new()));
    for (name, value) in [
        (LIST_SLOT_TABS, tabs),
        (LIST_SLOT_ZONES, zones.clone()),
        (LIST_SLOT_ZONES_2, zones),
        (LIST_SLOT_LEGEND, legend),
        // Explicitly empty, so the toast stack can never SSR a stale report.
        (LIST_SLOT_TOASTS, SlotValue::Array(Vec::new())),
    ] {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    for (id, value) in named_slot_ids(module, SHOW_SLOT_NAME)
        .into_iter()
        .zip(show_values(payload, selected, flash))
    {
        slots.set(id, SlotValue::Bool(value));
    }
    slots
}

/// Same anti-flash CSS as the status page (render.rs PERSONALITY_CSS).
const PERSONALITY_CSS: &str = "body{background:#0b0e14;color:#dbe2ef;margin:0}\
@media (prefers-color-scheme:light){body{background:#f2f4f8;color:#1a2130}}";

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
    render_page(&PageConfig {
        title: "ksx Studio — mapper",
        route_pattern: "/map",
        manifest: &page.manifest,
        config_script: None,
        body_class: None,
        personality_css: Some(PERSONALITY_CSS),
        body_prefix: Some(&prefix),
        render_mode: RenderMode::Phase2SsrReconcile,
        ir_module: Some(&page.module),
        slots: Some(&slots),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{LearnView, SessionView};
    use crate::snapshot::MapperSnapshot;
    use std::collections::BTreeMap;

    fn slot(number: u8, persona: &str, preset: &str) -> MapperSlot {
        let mut bindings = BTreeMap::new();
        bindings.insert("A".to_owned(), vec!["G".to_owned()]);
        bindings.insert("B".to_owned(), vec!["F".to_owned()]);
        bindings.insert("lx.min".to_owned(), vec!["M".to_owned()]);
        bindings.insert("start".to_owned(), Vec::new()); // cleared → unbound
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
        }
    }

    pub(super) fn sample() -> MapPayload {
        MapPayload {
            mapper: MapperSnapshot {
                generated_at: "2026-08-05 12:00:00 UTC".into(),
                source: "slots of profile \"Steam\" (games.toml)".into(),
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
        }
    }

    fn page() -> EmbeddedPage {
        EmbeddedPage::load("/map").expect("embedded mapper page must load")
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

        let scalars = scalar_slots(&sample(), None, None);
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
                LIST_SLOT_TOASTS
            ],
            "mapper list slot names drifted; slots: {names:?}"
        );
        assert_eq!(
            named_slot_ids(module, SHOW_SLOT_NAME).len(),
            MAP_SHOW_ORDER.len(),
            "mapper show count drifted between MapIsland.ts and MAP_SHOW_ORDER"
        );
        let islands = module.islands.entries();
        assert_eq!(islands.len(), 1, "expected exactly one island");
        assert_eq!(
            module.strings.get(islands[0].name_str_idx).unwrap(),
            ISLAND_COMPONENT
        );
        assert!(
            islands[0].slot_ids.is_empty(),
            "ledger #8 flipped — adopt native props"
        );
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
            Some("▲"),
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
            Some("▲"),
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
        // Nothing about it is styled or worded as an error.
        assert!(!out.html.contains("conflict"), "{}", out.html);
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
        assert!(!out.html.contains("disabled"), "{}", out.html);
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
            scalar_slots(&sample(), None, None)
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

    /// Ledger #5 parity, mapper edition: island props ARE the /api/map
    /// payload.
    #[test]
    fn island_props_match_the_api_map_payload_shape() {
        let payload = sample();
        let props = crate::render::island_props_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&props).expect("props parse");
        assert_eq!(
            parsed["0"],
            serde_json::to_value(&payload).unwrap(),
            "island props must be byte-compatible with what /api/map serves"
        );
        let out = render_map(&page(), &payload, None);
        assert!(out.html.contains(&props), "{}", out.html);
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
            26,
            "25 row forms + the bind-by-name panel: {html}"
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
            26,
            "every bind form offers Clear without JavaScript: {html}"
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
