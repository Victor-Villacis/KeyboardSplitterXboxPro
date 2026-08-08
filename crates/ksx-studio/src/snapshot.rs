//! The status contract — **now `ksx-api`'s** — plus the two PAGE payloads,
//! which stay here because they are this page's shape and nobody else's.
//!
//! `StatusSource` and its snapshots moved to `ksx-api` for the reason
//! docs/M9-DECISION.md §6 gives: the read side must be satisfiable with NO
//! daemon running (ksx-app's collectors read the config store and the platform
//! directly), and it is consumed by surfaces that do not link this crate. What
//! remains below is the part that genuinely belongs to a web page: the
//! envelope the islands protocol serializes into the document and the poller
//! reads back.

pub use ksx_api::status::*;

use serde::{Deserialize, Serialize};

/// The one live-data shape: what `GET /api/status` serves AND what the page
/// embeds (render.rs serializes it into the `__ksx-payload` script block).
/// One struct, one serializer — the client seeds its signals from the block
/// and then overwrites the SAME signals from `/api/status` every 2 s, so the
/// two must never drift. `render.rs` has the parity test;
/// `studio-ui/src/StatusIsland.ts` mirrors the field names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPayload {
    pub snapshot: StatusSnapshot,
    pub session: crate::control::SessionView,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/status` — a poll is not an action — and `Some` only in the
    /// page-render props, where the client shows it once and clears it.
    pub flash: Option<String>,
}

/// What `GET /api/map` serves AND what the mapper island's props carry — the
/// same one-struct-one-serializer rule as [`StatusPayload`], parity pinned in
/// `render_map.rs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapPayload {
    pub mapper: MapperSnapshot,
    pub session: crate::control::SessionView,
    /// Where the daemon's learner stands (also tells the page whether
    /// learning is possible at all).
    pub learn: crate::control::LearnView,
    /// Slot number selected for the SSR paint (`/map?slot=N`, defaulting to
    /// the first slot). The client keeps its own selection afterwards.
    pub selected: u8,
    /// The selected slot's macros, read per request like everything else.
    #[serde(default)]
    pub macros: MacroSnapshot,
    /// Macro name selected for the SSR paint (`/map?macro=NAME`), empty for
    /// "the first one". Same contract as [`selected`](Self::selected): it
    /// drives the server paint, the client keeps its own choice afterwards —
    /// and because the macro tabs are anchors, a page with no JavaScript can
    /// still walk through every macro the preset defines.
    #[serde(default)]
    pub macro_selected: String,
}

/// What `GET /api/devices` serves AND what the `/devices` island's props
/// carry — the same one-struct-one-serializer rule as [`StatusPayload`],
/// parity pinned in `render_devices.rs`.
///
/// The scan and the reason it is missing are SEPARATE fields on purpose. An
/// empty `DeviceScanView` is a real answer on a machine with nothing plugged
/// in; it is also what a refusal would degrade to if the two were collapsed,
/// and "no boards found" on a machine with four boards is the worst possible
/// lie for this page to tell. [`Self::unavailable`] non-empty means the view
/// below is not a reading of anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesPayload {
    pub scan: ksx_api::DeviceScanView,
    /// Session state, for the header pill and for the one caution this page
    /// owes a running cabinet: a `[[device]]` edit lands in `config.toml`, and
    /// the session already running keeps the devices it opened until it is
    /// restarted.
    pub session: crate::control::SessionView,
    /// Empty when the scan answered. Otherwise the refusal, verbatim.
    #[serde(default)]
    pub unavailable: String,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/devices` — a poll is not an action.
    pub flash: Option<String>,
}

/// What `GET /api/profiles` serves AND what the Profiles island's props carry
/// — the same one-struct-one-serializer rule as [`StatusPayload`], parity
/// pinned in `render_profiles.rs`.
///
/// Two machine views side by side rather than one flattened shape, because
/// they are two backend reads with two failure modes: games.toml can be
/// unreadable while the presets folder is fine, and the page has to be able to
/// say which. [`notes`](Self::notes) is where either read's complaint lands —
/// a refusal renders as a note beside an empty list, never as an empty list on
/// its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesPayload {
    pub profiles: ksx_api::ProfilesView,
    pub presets: ksx_api::PresetsView,
    pub session: crate::control::SessionView,
    /// The refusal that stopped the games.toml read, if it stopped.
    ///
    /// This field is the difference between two sentences that a count cannot
    /// tell apart: **"you have no profiles"** and **"I could not read your
    /// profiles."** Before it existed the handler substituted a
    /// `ProfilesView::default()` on `Err`, so an unreadable games.toml printed
    /// "no profiles in games.toml" at the top of the page with the real reason
    /// four cards further down. That is this project's signature failure —
    /// a surface reporting success over a read that did not happen — and it is
    /// the exact thing the rest of this page was written to stop.
    ///
    /// `Some` means the list below is empty BECAUSE THE READ FAILED, and every
    /// derived line ([`ProfilesDerived`]) says so instead of counting.
    #[serde(default)]
    pub profiles_error: Option<String>,
    /// The same, for the presets folder. Kept separate because the two reads
    /// fail independently and the page has to be able to say which.
    #[serde(default)]
    pub presets_error: Option<String>,
    /// Anything either read had to say out loud, including a whole read that
    /// refused. Rendered; never swallowed.
    #[serde(default)]
    pub notes: Vec<String>,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/profiles` — a poll is not an action.
    pub flash: Option<String>,
    /// Every displayed string and every branch this page needs, computed ONCE
    /// — see [`ProfilesDerived`]. Recomputed from the fields above by
    /// [`Self::derived`]; never assembled by hand.
    #[serde(default)]
    pub view: ProfilesDerived,
}

impl ProfilesPayload {
    /// Fill [`Self::view`] from the raw provider data.
    ///
    /// Every producer of a payload calls this — the page render and
    /// `GET /api/profiles` — so the server paint and the 2 s poll are the same
    /// bytes by construction rather than by two implementations agreeing.
    #[must_use]
    pub fn derived(mut self) -> Self {
        self.view = ProfilesDerived::of(&self);
        self
    }
}

/// Everything the Profiles page DISPLAYS that is not verbatim provider data:
/// the summary lines, the row lines, the pill classes, the option lists, the
/// slot ceiling, and every `show:` branch.
///
/// # Why it is a serialized struct and not two functions
///
/// It was two functions, and that was the review finding. Every line below
/// existed twice — once in `render_profiles.rs` for the server paint, once in
/// `ProfilesIsland.ts` for the 2 s poll — which docs/SURFACES.md §1 forbids
/// for exactly the reason it went wrong here: the TypeScript half carried a
/// hardcoded `"16"` slot ceiling that no poll could correct, so the first
/// `ksx_core::MAX_SLOTS` raise would have had the server render `max="32"` and
/// hydration write `16` straight back over it. Two copies of a SENTENCE drift
/// silently; two copies of a NUMBER drift silently and then refuse a legal
/// input. `main.rs`'s `slot_arg` module exists to commemorate the same bug.
///
/// So the derivation happens here, once, in the backend, and both the SSR slot
/// injection and the browser read the result. The island computes nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesDerived {
    /// The line above the profile list. Says "could not be read" — NOT
    /// "no profiles" — when [`ProfilesPayload::profiles_error`] is set.
    pub profiles_summary: String,
    pub broken_summary: String,
    pub presets_summary: String,
    pub templates_summary: String,
    /// The sentence above the template list, roster included. It was static
    /// copy in `ProfilesIsland.ts` naming four templates while the registry
    /// ships six — the copy-is-logic drift docs/SURFACES.md §1a is about,
    /// already stale on the day it was reviewed. Composed here from the same
    /// list the rows and the `<select>` options are built from.
    pub templates_intro: String,
    /// The exact `ksx daemon …` line for this cabinet.
    pub daemon_cmd: String,
    /// `ksx_core::MAX_SLOTS`, as the slot-count input's `max`. The ONE place
    /// this number may come from; a client-side literal was the finding.
    pub max_slots: u8,
    /// The widest player block any offered template carries — the preset
    /// form's `max`, which used to be the literal `"4"` whether or not the
    /// selected template had four blocks.
    pub max_player: u8,
    pub profile_rows: Vec<ProfileRowView>,
    pub broken_rows: Vec<BrokenRowView>,
    pub preset_rows: Vec<PresetRowView>,
    /// The in-box templates as a LIST, carrying `detail` — the panel note
    /// ksx-api documents as the thing that makes a template identifiable.
    /// Served since the beginning and rendered nowhere until now.
    pub template_rows: Vec<TemplateRowView>,
    pub preset_options: Vec<OptionView>,
    pub template_options: Vec<OptionView>,
    pub note_rows: Vec<NoteView>,

    // ── The `show:` branches. Booleans, because a page that decides in two
    //    languages decides differently in one of them.
    pub pill_running: bool,
    pub pill_idle: bool,
    pub pill_down: bool,
    pub no_daemon: bool,
    pub any_broken: bool,
    /// Offer the Switch button (a start could actually be accepted).
    pub rows_live: bool,
    pub rows_plain: bool,
    /// The games.toml read REFUSED. Distinct from "no profiles" on purpose.
    pub profiles_unreadable: bool,
    /// The create-profile form is usable: presets were read, and there is one.
    pub can_make_profile: bool,
    /// Presets were read and there are none — a real, fixable empty state
    /// whose copy points at the template form below, which will work.
    pub no_presets_yet: bool,
    /// The presets read REFUSED. NOT [`Self::no_presets_yet`]: that sentence
    /// sends the user to a template form whose `<select>` is also empty, so
    /// the only path it offers cannot succeed — a closed loop with a wrong
    /// sentence on it.
    pub presets_unreadable: bool,
    /// The template form is usable at all (the presets read, which carries the
    /// template list, succeeded).
    pub can_make_preset: bool,
    pub any_notes: bool,
}

/// One `[[game]]` profile as a row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRowView {
    pub title: String,
    pub path: String,
    pub detail: String,
    pub verdict: String,
    /// The pill class. Derived from `ProfileDetail::state` HERE so the pill a
    /// poll paints is the pill the server painted.
    pub statecls: String,
    pub statelabel: String,
}

/// One broken profile in the alarm card.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokenRowView {
    pub title: String,
    /// The path that does not resolve — the whole reason the card exists.
    pub path: String,
    pub verdict: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetRowView {
    pub name: String,
    pub detail: String,
    pub statecls: String,
    pub statelabel: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRowView {
    pub id: String,
    pub label: String,
    /// The panel note that travels with the template.
    pub detail: String,
    /// "player 1" / "players 1–2" — the block range this template can
    /// instantiate, so the number the form asks for is visible next to it.
    pub players: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionView {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteView {
    pub line: String,
}

impl ProfilesDerived {
    /// Derive the whole display layer from one payload.
    fn of(p: &ProfilesPayload) -> Self {
        let profiles_failed = p.profiles_error.is_some();
        let presets_failed = p.presets_error.is_some();
        let has_presets = !p.presets.presets.is_empty();
        let can_start = p.session.reachable && !p.session.running;

        // The provider's word, never a re-derivation: deciding what counts as
        // broken outside the provider is what docs/SURFACES.md §1 forbids.
        let broken: Vec<&ksx_api::ProfileDetail> = p
            .profiles
            .profiles
            .iter()
            .filter(|g| g.state == "broken")
            .collect();

        Self {
            profiles_summary: profiles_summary(p.profiles.profiles.len(), profiles_failed),
            broken_summary: broken_summary(broken.len()),
            presets_summary: presets_summary(p.presets.presets.len(), presets_failed),
            templates_summary: templates_summary(p.presets.templates.len(), presets_failed),
            templates_intro: templates_intro(&p.presets.templates, presets_failed),
            daemon_cmd: crate::render::daemon_command(&p.session),
            max_slots: ksx_api::MAX_SLOTS,
            max_player: p
                .presets
                .templates
                .iter()
                .filter_map(|t| t.players.iter().copied().max())
                .max()
                .unwrap_or(1),
            profile_rows: p
                .profiles
                .profiles
                .iter()
                .map(|g| ProfileRowView {
                    title: g.title.clone(),
                    path: g.path.clone(),
                    detail: profile_detail_line(g),
                    verdict: g.verdict.clone(),
                    statecls: state_class(&g.state).to_owned(),
                    statelabel: g.state.clone(),
                })
                .collect(),
            broken_rows: broken
                .iter()
                .map(|g| BrokenRowView {
                    title: g.title.clone(),
                    // `broken_path` is the provider's answer to "which string
                    // is wrong"; falling back to `path` keeps the row honest
                    // for the empty-path case, where there IS no bad path.
                    path: g.broken_path.clone().unwrap_or_else(|| g.path.clone()),
                    verdict: g.verdict.clone(),
                })
                .collect(),
            preset_rows: p
                .presets
                .presets
                .iter()
                .map(|r| PresetRowView {
                    name: r.name.clone(),
                    detail: preset_detail_line(r),
                    statecls: if r.protected {
                        "pill pill-idle".to_owned()
                    } else {
                        "pill pill-ok".to_owned()
                    },
                    statelabel: if r.protected {
                        "built-in".to_owned()
                    } else {
                        "yours".to_owned()
                    },
                })
                .collect(),
            template_rows: p
                .presets
                .templates
                .iter()
                .map(|t| TemplateRowView {
                    id: t.id.clone(),
                    label: t.label.clone(),
                    detail: t.detail.clone(),
                    players: player_range(&t.players),
                })
                .collect(),
            preset_options: p
                .presets
                .presets
                .iter()
                .map(|r| OptionView {
                    value: r.name.clone(),
                    label: r.name.clone(),
                })
                .collect(),
            template_options: p
                .presets
                .templates
                .iter()
                .map(|t| OptionView {
                    value: t.id.clone(),
                    // The player range is IN the option, because the form's
                    // player field is one ceiling for every template and the
                    // user is the only one who can see which they picked.
                    label: format!("{} — {} ({})", t.id, t.label, player_range(&t.players)),
                })
                .collect(),
            note_rows: p
                .notes
                .iter()
                .map(|line| NoteView { line: line.clone() })
                .collect(),

            pill_running: p.session.reachable && p.session.running,
            pill_idle: can_start,
            pill_down: !p.session.reachable,
            no_daemon: !p.session.reachable,
            any_broken: !broken.is_empty(),
            rows_live: can_start,
            rows_plain: !can_start,
            profiles_unreadable: profiles_failed,
            can_make_profile: has_presets && !presets_failed,
            no_presets_yet: !has_presets && !presets_failed,
            presets_unreadable: presets_failed,
            can_make_preset: !presets_failed,
            any_notes: !p.notes.is_empty(),
        }
    }
}

/// The line above the profile list.
///
/// The `failed` arm is the point of this function. "no profiles in games.toml"
/// is a statement about the file's CONTENTS; when the read refused, nothing is
/// known about the contents, and printing the count sentence asserts an
/// absence nobody checked.
fn profiles_summary(count: usize, failed: bool) -> String {
    if failed {
        return "games.toml could NOT be read — this is not an empty list, it is a failed \
                read, and the reason is below"
            .to_owned();
    }
    match count {
        0 => "no profiles in games.toml".to_owned(),
        1 => "1 profile in games.toml:".to_owned(),
        n => format!("{n} profiles in games.toml:"),
    }
}

fn broken_summary(count: usize) -> String {
    match count {
        1 => "1 profile points at a program that is not there:".to_owned(),
        n => format!("{n} profiles point at a program that is not there:"),
    }
}

fn presets_summary(count: usize, failed: bool) -> String {
    if failed {
        return "the presets could NOT be read — this is not an empty folder, it is a failed \
                read, and the reason is below"
            .to_owned();
    }
    match count {
        0 => "no presets on disk".to_owned(),
        1 => "1 preset on disk:".to_owned(),
        n => format!("{n} presets on disk:"),
    }
}

/// The templates ship inside the binary, so an empty list here means the read
/// that carries them refused — never "this build has no templates".
fn templates_summary(count: usize, failed: bool) -> String {
    if failed {
        return "the in-box templates could not be listed — the presets read refused".to_owned();
    }
    match count {
        0 => "no in-box templates".to_owned(),
        1 => "1 in-box template:".to_owned(),
        n => format!("{n} in-box templates:"),
    }
}

/// The intro sentence for the template card, ROSTER INCLUDED.
///
/// The roster was static copy in the island — "an I-PAC on its factory chart,
/// MAME's four-player chart, a desk keyboard, and two players sharing one
/// keyboard" — which names four templates. `ksx_core::templates::TEMPLATES`
/// ships six; `default` and `empty` are templates too, and both were already
/// in the form's `<select>` below the sentence that omitted them. That is
/// docs/SURFACES.md §1a drift that had ALREADY happened, in the very change
/// that added §1a. The ids are the roster here because they are the string
/// the rows below lead with and the value the `<select>` submits.
///
/// When the read that carries the templates refused, the sentence claims no
/// roster at all: enumerating from a failed read would be §1b's bug in copy.
fn templates_intro(templates: &[ksx_api::TemplateRow], failed: bool) -> String {
    const CLOSE: &str =
        "Instantiating one writes an ordinary, editable preset file; from then on it is yours.";
    if failed || templates.is_empty() {
        return format!("The layouts that ship in the binary. {CLOSE}");
    }
    let ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
    let roster = match ids.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => ids.concat(),
    };
    format!("The layouts that ship in the binary — {roster}. {CLOSE}")
}

/// A profile's state as the pill class that carries it.
///
/// `launcher` gets the NEUTRAL pill, not the OK one, and that is not a style
/// choice: `ksx_games::preflight` cannot resolve a `steam://` URL — only the
/// shell knows whether `rungameid/9999` names a real game — so a green badge
/// would be ksx claiming a check it did not make.
fn state_class(state: &str) -> &'static str {
    match state {
        "broken" => "pill pill-warn",
        "launcher" => "pill pill-idle",
        _ => "pill pill-ok",
    }
}

fn profile_detail_line(p: &ksx_api::ProfileDetail) -> String {
    let slots = match p.slots {
        1 => "1 slot".to_owned(),
        n => format!("{n} slots"),
    };
    if p.presets.is_empty() {
        format!("{slots} — no preset named")
    } else {
        format!("{slots} on {}", p.presets.join(", "))
    }
}

fn preset_detail_line(p: &ksx_api::PresetRow) -> String {
    let controls = match p.bound {
        1 => "1 control".to_owned(),
        n => format!("{n} controls"),
    };
    let macros = match p.macros {
        0 => String::new(),
        n => format!(", {n} macro(s)"),
    };
    format!("{controls}{macros} — {}", p.source)
}

/// "player 1" / "players 1–3" — the blocks a template can instantiate.
fn player_range(players: &[u8]) -> String {
    match (players.iter().min(), players.iter().max()) {
        (Some(lo), Some(hi)) if lo == hi => format!("player {lo}"),
        (Some(lo), Some(hi)) => format!("players {lo}–{hi}"),
        _ => "no player blocks".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The template-card intro's roster is DERIVED, never copy. Every offered
    /// template's id is in the sentence — so a seventh template shows up
    /// without anyone remembering prose — and a FAILED read enumerates
    /// nothing, because a roster composed from a read that refused would be
    /// §1b's bug wearing §1a's clothes.
    #[test]
    fn the_template_intro_names_every_offered_template_and_none_on_a_failed_read() {
        let row = |id: &str| ksx_api::TemplateRow {
            id: id.to_owned(),
            label: String::new(),
            detail: String::new(),
            players: vec![1],
        };
        let templates = [row("arcade-6button"), row("keyboard-2p"), row("empty")];

        let intro = templates_intro(&templates, false);
        for t in &templates {
            assert!(intro.contains(&t.id), "{} missing from: {intro}", t.id);
        }
        assert!(
            intro.contains("keyboard-2p and empty"),
            "the roster reads as a sentence: {intro}"
        );

        let refused = templates_intro(&templates, true);
        for t in &templates {
            assert!(
                !refused.contains(&t.id),
                "a failed read must not enumerate templates: {refused}"
            );
        }
    }

    /// The payload's field names are client contract (StatusIsland.ts reads
    /// them); pin the envelope on top of the snapshot's own pinned names
    /// (`ksx-api`'s `status` module keeps those).
    #[test]
    fn payload_serializes_to_stable_envelope_field_names() {
        let payload = StatusPayload {
            snapshot: StatusSnapshot {
                generated_at: "2026-08-04 12:00:00 UTC".into(),
                ..StatusSnapshot::default()
            },
            session: crate::control::SessionView {
                reachable: true,
                running: false,
                line: "idle — daemon reachable".into(),
                profile: None,
            },
            flash: None,
        };
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            v.pointer("/snapshot/generated_at"),
            Some(&serde_json::json!("2026-08-04 12:00:00 UTC"))
        );
        assert_eq!(
            v.pointer("/session/reachable"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/session/running"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            v.pointer("/session/line"),
            Some(&serde_json::json!("idle — daemon reachable"))
        );
        // `flash` is always present (null when absent) — the client types it
        // `string | null`, not optional.
        assert_eq!(v.pointer("/flash"), Some(&serde_json::json!(null)));
    }
}
