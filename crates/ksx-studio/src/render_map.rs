//! The mapper page's render seam (`/map`): embedded FMIR + per-request
//! [`MapPayload`] → HTML. Same architecture as `render.rs` (SSR slots for
//! first paint, the identical payload as island props for hydration — dogfood
//! ledger #5), same compiler constraints, its own slot layout.
//!
//! # The zone model
//!
//! The controller art (Gamepad-Asset-Pack, MIT — see `studio-ui/art/README.md`)
//! is an `<img>` filling the bottom `ART_SHARE` of a fixed-aspect "stage";
//! the top band is a shoulder shelf for LB/RB/LT/RT, which the icon art does
//! not draw. Every mappable control is a HIT ZONE: an absolutely positioned
//! `<button data-fn=…>` from the [`ZONE_XBOX`]/[`ZONE_DS4`] tables, carrying
//! its label and its current binding as a key tag. One `createList` renders
//! all 25 zones; geometry is data, not markup.
//!
//! Zone coordinates are STAGE percentages, authored from the art's real
//! geometry (`studio-ui/art/extents.mjs` — the PadForge lesson: derive layout
//! from art with a script, never trace by eye) plus hand placement for
//! controls the icons do not draw (start/back/guide, shoulders). The tables
//! are mirrored in `studio-ui/src/MapIsland.ts` (client re-derivation per
//! poll — the established applyStatus pattern); `zone_tables_cover_every_
//! mappable_function` pins this side.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{art_for, body_prefix, EmbeddedPage};
use crate::snapshot::{MapPayload, MapperSlot};

/// List slot names (binding-derived, compiler 0.2.0). The zones list appears
/// TWICE — once inside each stage show (xbox / ds4) — so the second
/// occurrence gets the `#2` suffix, exactly like the status page's
/// profileRows pair; both receive the same array.
const LIST_SLOT_TABS: &str = "list:slotTabs:array";
const LIST_SLOT_ZONES: &str = "list:zones:array";
const LIST_SLOT_ZONES_2: &str = "list:zones#2:array";

#[cfg(test)]
const ISLAND_COMPONENT: &str = "MapIsland";

/// The mapper's positional `show:createShow` seam (ledger #4), document order
/// in MapIsland.ts.
const SHOW_SLOT_NAME: &str = "show:createShow";
const MAP_SHOW_ORDER: [&str; 12] = [
    "header pill: running",
    "header pill: idle",
    "header pill: no daemon",
    "read-only banner + CLI fallback",
    "hint: click-a-control (learnable)",
    "stage: xbox art (+ zone layer)",
    "stage: ds4 art (+ zone layer)",
    "saved flash: ok",
    "saved flash: error",
    "modal: overlay open",
    "modal: listening (countdown)",
    "modal: conflict (Replace / Cancel)",
];
const MAP_SHOW_COUNT: usize = MAP_SHOW_ORDER.len();

// The art `<img>` occupies the bottom 86% of the stage (`.padart` in
// studio.css); the top band is the shoulder shelf. Zone Y values below are
// authored as `14 + artY·0.86`.

/// One hit zone: canonical function, on-art label, stage-percent box, css
/// variant.
pub(crate) struct Zone {
    pub fn_name: &'static str,
    pub label: &'static str,
    /// Center, stage percent.
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// CSS variant class: round | chip | shoulder.
    pub kind: &'static str,
}

const fn zone(
    fn_name: &'static str,
    label: &'static str,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    kind: &'static str,
) -> Zone {
    Zone {
        fn_name,
        label,
        cx,
        cy,
        w,
        h,
        kind,
    }
}

/// Xbox-series-style pad (art: `pad-xbox.svg`, viewBox 112.46×76.66; anchors
/// from extents.mjs: face Y(75.2,19.9) B(82.0,29.8) A(75.3,39.9) X(68.7,29.9),
/// Lstick(24.0,29.9), Rstick(62.5,51.6), dpad(36.4,53.4) — art Y mapped to
/// stage as 14 + y·0.86).
pub(crate) const ZONE_XBOX: [Zone; 25] = [
    // Shoulder shelf (not drawn in the icon art): triggers outboard.
    zone("lt", "LT", 12.0, 6.5, 11.5, 9.5, "shoulder"),
    zone("lb", "LB", 25.5, 6.5, 11.5, 9.5, "shoulder"),
    zone("rb", "RB", 74.5, 6.5, 11.5, 9.5, "shoulder"),
    zone("rt", "RT", 88.0, 6.5, 11.5, 9.5, "shoulder"),
    // Face cluster.
    zone("Y", "Y", 75.2, 31.1, 9.0, 11.5, "round"),
    zone("B", "B", 82.0, 39.6, 9.0, 11.5, "round"),
    zone("A", "A", 75.3, 48.3, 9.0, 11.5, "round"),
    zone("X", "X", 68.7, 39.7, 9.0, 11.5, "round"),
    // Center row: guide up top, view/menu inboard.
    zone("guide", "⌂", 50.0, 26.5, 10.0, 12.0, "round"),
    zone("back", "view", 43.0, 40.0, 7.5, 8.5, "chip"),
    zone("start", "menu", 57.0, 40.0, 7.5, 8.5, "chip"),
    // Left stick + its four directions.
    zone("lthumb", "L3", 24.0, 39.7, 9.0, 11.5, "round"),
    zone("ly.max", "▲", 24.0, 27.7, 8.0, 10.0, "chip"),
    zone("ly.min", "▼", 24.0, 51.7, 8.0, 10.0, "chip"),
    zone("lx.min", "◀", 14.5, 39.7, 8.0, 10.0, "chip"),
    zone("lx.max", "▶", 33.5, 39.7, 8.0, 10.0, "chip"),
    // Dpad cross.
    zone("dpad.up", "▲", 36.4, 50.6, 7.0, 9.0, "chip"),
    zone("dpad.down", "▼", 36.4, 69.2, 7.0, 9.0, "chip"),
    zone("dpad.left", "◀", 29.2, 59.9, 7.0, 9.0, "chip"),
    zone("dpad.right", "▶", 43.6, 59.9, 7.0, 9.0, "chip"),
    // Right stick + directions.
    zone("rthumb", "R3", 62.5, 58.4, 9.0, 11.5, "round"),
    zone("ry.max", "▲", 61.0, 47.0, 7.0, 8.5, "chip"),
    zone("ry.min", "▼", 62.5, 70.4, 8.0, 10.0, "chip"),
    zone("rx.min", "◀", 53.0, 58.4, 8.0, 10.0, "chip"),
    zone("rx.max", "▶", 72.0, 59.5, 7.0, 9.5, "chip"),
];

/// DualShock 4 pad (art: `pad-ds4.svg`, viewBox 112.69×72.53; anchors:
/// △(81.2,17.7) ○(88.4,28.8) ✕(81.3,39.7) □(74.0,28.7), sticks (33.8,49.8)
/// and (66.1,49.8), dpad arrows around (18.5,29.3), touchpad x 32.9..67.0 —
/// Sony labels, XInput functions).
pub(crate) const ZONE_DS4: [Zone; 25] = [
    zone("lt", "L2", 8.0, 6.5, 10.0, 9.5, "shoulder"),
    zone("lb", "L1", 20.5, 6.5, 10.0, 9.5, "shoulder"),
    zone("rb", "R1", 79.5, 6.5, 10.0, 9.5, "shoulder"),
    zone("rt", "R2", 92.0, 6.5, 10.0, 9.5, "shoulder"),
    // Face cluster (✕○△□ mapped onto A/B/Y/X).
    zone("Y", "△", 81.2, 29.2, 8.5, 11.0, "round"),
    zone("B", "○", 88.4, 38.8, 8.5, 11.0, "round"),
    zone("A", "✕", 81.3, 48.1, 8.5, 11.0, "round"),
    zone("X", "□", 74.0, 38.7, 8.5, 11.0, "round"),
    // Share / PS / Options.
    zone("back", "share", 30.0, 25.5, 7.0, 9.0, "chip"),
    zone("start", "options", 70.0, 25.5, 7.0, 9.0, "chip"),
    zone("guide", "PS", 50.0, 63.0, 8.0, 10.0, "round"),
    // Left stick + directions.
    zone("lthumb", "L3", 33.8, 56.8, 9.0, 11.0, "round"),
    zone("ly.max", "▲", 33.8, 45.0, 7.5, 9.0, "chip"),
    zone("ly.min", "▼", 33.8, 68.6, 7.5, 9.5, "chip"),
    zone("lx.min", "◀", 24.0, 56.8, 7.5, 9.5, "chip"),
    zone("lx.max", "▶", 43.6, 56.8, 7.5, 9.5, "chip"),
    // Dpad arrows.
    zone("dpad.up", "▲", 18.5, 32.5, 7.0, 9.0, "chip"),
    zone("dpad.down", "▼", 18.5, 45.6, 7.0, 9.0, "chip"),
    zone("dpad.left", "◀", 13.5, 39.2, 7.0, 9.0, "chip"),
    zone("dpad.right", "▶", 23.3, 39.2, 7.0, 9.0, "chip"),
    // Right stick + directions.
    zone("rthumb", "R3", 66.1, 56.8, 9.0, 11.0, "round"),
    zone("ry.max", "▲", 66.1, 45.0, 7.0, 8.5, "chip"),
    zone("ry.min", "▼", 66.1, 68.6, 7.5, 9.5, "chip"),
    zone("rx.min", "◀", 56.4, 56.8, 7.5, 9.5, "chip"),
    zone("rx.max", "▶", 74.8, 58.2, 7.0, 9.5, "chip"),
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

/// "G", "S+Enter", or "—" for unbound. The inert "None" placeholder was
/// already filtered by the provider.
fn key_tag(slot: &MapperSlot, function: &str) -> String {
    match slot.bindings.get(function) {
        Some(keys) if !keys.is_empty() => keys.join("+"),
        _ => "—".to_owned(),
    }
}

/// Mirrors MapIsland.ts `zoneRows`. Every derived string the client also
/// derives; the item SHAPE is client contract.
fn zone_rows(slot: &MapperSlot) -> SlotValue {
    SlotValue::Array(
        zones_for(&slot.persona)
            .iter()
            .map(|z| {
                let key = key_tag(slot, z.fn_name);
                let unbound = key == "—";
                SlotValue::Object(vec![
                    ("fn".to_owned(), SlotValue::Text(z.fn_name.to_owned())),
                    ("label".to_owned(), SlotValue::Text(z.label.to_owned())),
                    ("key".to_owned(), SlotValue::Text(key.clone())),
                    (
                        "cls".to_owned(),
                        SlotValue::Text(format!(
                            "zone z-{}{}",
                            z.kind,
                            if unbound { " z-unbound" } else { "" }
                        )),
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
                        SlotValue::Text(format!("{} — {}", z.fn_name, key)),
                    ),
                ])
            })
            .collect(),
    )
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
        return "read-only while a session is running: captured keys never reach the learner. \
                Stop the session to map here, or bind from a shell with the command below \
                (then Reload)"
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

fn scalar_slots(payload: &MapPayload, selected: Option<&MapperSlot>) -> serde_json::Value {
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
        "modalPrompt": "",
        "countdownText": "",
        "barStyle": "width:100%",
        "conflictLine": "",
        "savedLine": "",
        "generatedAt": payload.mapper.generated_at,
    })
}

fn show_values(payload: &MapPayload, selected: Option<&MapperSlot>) -> [bool; MAP_SHOW_COUNT] {
    let art = selected.map(|s| art_for(&s.persona));
    let live = learnable(payload) && selected.is_some();
    [
        payload.session.reachable && payload.session.running,
        payload.session.reachable && !payload.session.running,
        !payload.session.reachable,
        !live,
        live,
        art == Some(crate::render::ART_XBOX),
        art == Some(crate::render::ART_DS4),
        false, // saved flashes: client-only
        false,
        false, // modal: client-only, never SSR-open
        false,
        false,
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

fn build_slots(module: &IrModule, payload: &MapPayload) -> SlotData {
    let selected = selected_slot(payload);
    let scalars = scalar_slots(payload, selected).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    let tabs = slot_tabs(payload, selected);
    let zones = selected
        .map(zone_rows)
        .unwrap_or(SlotValue::Array(Vec::new()));
    for (name, value) in [
        (LIST_SLOT_TABS, tabs),
        (LIST_SLOT_ZONES, zones.clone()),
        (LIST_SLOT_ZONES_2, zones),
    ] {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    for (id, value) in named_slot_ids(module, SHOW_SLOT_NAME)
        .into_iter()
        .zip(show_values(payload, selected))
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
pub(crate) fn render_map(page: &EmbeddedPage, payload: &MapPayload) -> PageOutput {
    let slots = build_slots(&page.module, payload);
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
        }
    }

    fn sample() -> MapPayload {
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

        let scalars = scalar_slots(&sample(), None);
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
            [LIST_SLOT_TABS, LIST_SLOT_ZONES, LIST_SLOT_ZONES_2],
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

    /// The big one: real bindings from the payload land as zone key tags in
    /// the SSR HTML, the right art is referenced, and the slot strip shows
    /// the context.
    #[test]
    fn render_puts_bindings_on_zones_and_the_art_on_stage() {
        let out = render_map(&page(), &sample());
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
        // Zones carry their data-fn and their binding tags (A→G from the
        // sample), positioned by the table.
        assert!(out.html.contains(r#"data-fn="A""#), "{}", out.html);
        assert!(out.html.contains(r#"data-fn="dpad.up""#), "{}", out.html);
        assert!(out.html.contains(">G<"), "A's key tag: {}", out.html);
        assert!(out.html.contains(">F<"), "B's key tag: {}", out.html);
        assert!(out.html.contains(">M<"), "lx.min's key tag: {}", out.html);
        // A cleared function renders the honest unbound tag.
        assert!(out.html.contains("z-unbound"), "{}", out.html);
        // Live mode: the hint renders, the read-only banner does not.
        assert!(out.html.contains("press the panel key"), "{}", out.html);
        assert!(!out.html.contains("read-only"), "{}", out.html);
    }

    /// `?slot=3` selects the PlayStation slot: DS4 art, Sony labels.
    #[test]
    fn selected_slot_drives_art_and_labels() {
        let mut payload = sample();
        payload.selected = 3;
        let out = render_map(&page(), &payload);
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
        let out = render_map(&page(), &payload);
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
    }

    /// A running session is read-only too (the learner cannot hear captured
    /// keys), and says so.
    #[test]
    fn a_running_session_renders_the_capture_explanation() {
        let mut payload = sample();
        payload.session.running = true;
        payload.session.line = "running — 4 pad(s)".into();
        let out = render_map(&page(), &payload);
        assert!(
            out.html.contains("captured keys never reach the learner"),
            "{}",
            out.html
        );
    }

    /// A daemon that predates the learn verbs: honest reason with the pipe's
    /// own error text.
    #[test]
    fn a_pre_mapper_daemon_is_reported_not_hidden() {
        let mut payload = sample();
        payload.learn = LearnView::unavailable("unknown verb 'learn-poll'");
        let out = render_map(&page(), &payload);
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
        let out = render_map(&page(), &payload);
        assert!(out.html.contains(&props), "{}", out.html);
    }

    /// The attribution promised in studio-ui/art/README.md is visibly on the
    /// page (both pages carry it; render.rs pins the status page).
    #[test]
    fn the_mapper_page_credits_the_controller_art() {
        let out = render_map(&page(), &sample());
        assert!(
            out.html.contains("Gamepad-Asset-Pack (MIT) by AL2009man"),
            "{}",
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
        let out = render_map(&page(), &payload);
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
    }
}
