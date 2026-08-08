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

/// What `GET /api/check` serves AND what the button-check island's props carry
/// — the same one-struct-one-serializer rule as [`StatusPayload`], parity
/// pinned in `render_check.rs`.
///
/// **There is no live data in here, and that is the shape of the page.** This
/// payload is the STRUCTURE — which slots exist, which controls each one's
/// preset names, which keys drive them — read from disk on the server and
/// re-read every few seconds. The lighting-up arrives on a different channel
/// entirely (`GET /api/live`, `crate::live`), at display rate, and touches no
/// signal on the page. Putting a frame in here would mean a button check whose
/// echo was as fast as an HTTP poll, which is not a button check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckPayload {
    /// The slot roster with every preset's whole binding table — the SAME
    /// `StatusSource::mapper()` read the mapper page uses.
    ///
    /// The control list per slot is `MapperSlot::bindings`' key set, which is
    /// every function the preset names, unbound ones included. That is where
    /// the roster has to come from: a list of "the controls an Xbox pad has"
    /// written into the page would be a second answer to a question the
    /// backend already answers, and docs/SURFACES.md §1 names that failure.
    pub mapper: MapperSnapshot,
    /// The daemon's session state — the page prints it, because "nothing is
    /// lighting up" and "nothing is running" are the same picture and
    /// different problems.
    pub session: crate::control::SessionView,
    /// One sentence saying what this screen watches and where the frames come
    /// from. Composed in Rust so the island words nothing.
    pub feed_hint: String,
}

/// What `GET /api/pads` serves AND what the pads island's props carry — the
/// same one-struct-one-serializer rule as [`StatusPayload`], parity pinned in
/// `render_pads.rs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadsPayload {
    /// The bus, its children, and both verbs' preconditions — one
    /// `MachineSource::pads_view` call, never re-derived here.
    pub pads: ksx_api::PadsView,
    /// Whether the daemon answers at all. Not a precondition for this page:
    /// the pad list and the prune plan are collector reads, and the session is
    /// shown because a running one is what REFUSES both verbs.
    pub session: crate::control::SessionView,
    /// Is the destructive panel armed (`/pads?confirm=1`)?
    ///
    /// Always `false` from `/api/pads` — a poll is not a user saying yes, and
    /// a poll that could re-arm a prune would make the confirm panel reappear
    /// after someone had deliberately navigated away from it.
    #[serde(default)]
    pub confirm: bool,
    /// Why the machine read failed, if it did. Rendered as a banner instead of
    /// an empty pad list, which would read as "your bus is clean".
    #[serde(default)]
    pub unavailable: Option<String>,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/pads`, `Some` only in the page-render props.
    pub flash: Option<String>,
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

/// The setup page's read: `ksx_api::MachineSource::setup_state`, plus the one
/// fact a bare `Result` cannot carry into a template — WHY there is nothing.
///
/// The same shape, and the same reason, as [`MapperSnapshot::unavailable`]: a
/// provider that refuses must produce a page that SAYS so, not an empty
/// checklist that looks like a machine with nothing configured. Those two
/// states are opposite advice ("import a config" vs "this build has no machine
/// provider") and a blank page gives the wrong one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupSnapshot {
    /// The machine provider answered.
    pub available: bool,
    /// Where the facts came from, or — with `available` false — why there are
    /// none. Rendered either way.
    pub source: String,
    pub view: ksx_api::SetupView,
}

impl SetupSnapshot {
    pub fn ready(view: ksx_api::SetupView) -> Self {
        Self {
            available: true,
            source: "read from this machine's config root".to_owned(),
            view,
        }
    }

    /// No setup facts; `reason` renders where the checklist would be.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            source: reason.to_owned(),
            view: ksx_api::SetupView::default(),
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

/// The sentence a refused read produces, everywhere it appears. A page that
/// could not read a machine says THIS; it never says the machine is empty.
const UNREADABLE: &str = "The configuration could not be read.";

/// **Every sentence `/setup` states as a fact, composed once, in Rust.**
///
/// docs/SURFACES.md §1, applied to the render seam: the SSR paint and the
/// island's two-second poll show the same words because there is only one
/// implementation of them, not two that a reviewer has to diff. `SetupIsland.ts`
/// reads these fields and renders them; it derives nothing. Six of these lines
/// were hand-mirrored in TypeScript until an adversarial review pointed out
/// that the test claiming to pin the two sides together only ever read the Rust
/// one.
///
/// The other half of the job is [`SetupSnapshot::available`]. A provider that
/// REFUSED knows nothing about this machine, so not one line below may assert
/// absence — "I could not read this" and "there is nothing here" are different
/// sentences and a user acts on them differently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupLines {
    /// The loudest line on the page: the whole configuration in one sentence.
    pub config: String,
    /// The inventory's board heading.
    pub boards: String,
    /// The inventory's slot heading.
    pub slots: String,
    /// What is on disk, in one line.
    pub library: String,
    /// What the Export button would hand back.
    pub export: String,
    /// Where the daemon's learner stands, as step 3 reads it.
    pub prove: String,
    /// WHY the wire-a-slot control is disabled, or empty when it is not.
    ///
    /// One reason, never a disjunction: `wireable` is two facts ANDed, and a
    /// single sentence covering both told a user with a running daemon to start
    /// the daemon, and a user with presets on disk to go and make a preset.
    pub wire_blocked: String,
    /// WHY the learner control is disabled, or empty when it is not. Same rule:
    /// "no daemon" and "a daemon whose listener is unavailable" are different
    /// states and the page knows which one it is in.
    pub prove_blocked: String,
    /// What wiring a slot will do to the pads, for the session there actually
    /// is. The unconditional "REPLUGS the pads" was false against an idle
    /// daemon — which the form is offered on, because `wireable` turns on
    /// `reachable`, not on `running`.
    pub wire_warning: String,
}

impl SetupLines {
    /// Compose every line for one read. The only implementation.
    pub fn of(
        setup: &SetupSnapshot,
        session: &crate::control::SessionView,
        learn: &crate::control::LearnView,
    ) -> Self {
        let view = &setup.view;
        if !setup.available {
            // NOT "there is nothing configured" — that sentence is advice
            // ("import one"), and it would be the wrong advice. Every line here
            // says the same thing the guard on the card says.
            return Self {
                config: UNREADABLE.to_owned(),
                boards: "the boards on this machine could not be read".to_owned(),
                slots: "the slots on this machine could not be read".to_owned(),
                library: "What is on disk could not be read — which is not the same as \
                          nothing being there."
                    .to_owned(),
                export: "Export hands back what this machine holds; ksx could not read it \
                         to say what that is."
                    .to_owned(),
                prove: learn_line(learn),
                wire_blocked: "disabled — this machine's configuration could not be read, \
                               so ksx cannot offer the presets a slot would point at."
                    .to_owned(),
                prove_blocked: prove_blocked_line(session, learn),
                wire_warning: wire_warning_line(session),
            };
        }

        Self {
            config: if view.config_exists {
                format!(
                    "Configured — {} board(s), {} slot(s), {} preset(s).",
                    view.devices.len(),
                    view.slots.len(),
                    view.presets.len()
                )
            } else {
                "There is no configuration on this machine yet.".to_owned()
            },
            boards: match view.devices.len() {
                0 => "no boards named yet".to_owned(),
                1 => "1 board named:".to_owned(),
                n => format!("{n} boards named:"),
            },
            slots: match view.slots.len() {
                0 => "no slots wired yet".to_owned(),
                1 => "1 slot wired:".to_owned(),
                n => format!("{n} slots wired:"),
            },
            library: format!(
                "{} preset(s) and {} game profile(s) on disk.",
                view.presets.len(),
                view.profiles.len()
            ),
            export: format!(
                "One JSON file: settings, boards, slots, {} game profile(s) and {} preset(s).",
                view.profiles.len(),
                view.presets.len()
            ),
            prove: learn_line(learn),
            wire_blocked: wire_blocked_line(session, view),
            prove_blocked: prove_blocked_line(session, learn),
            wire_warning: wire_warning_line(session),
        }
    }
}

/// The learner, as the sentence step 3 reads.
fn learn_line(learn: &crate::control::LearnView) -> String {
    match learn.state.as_str() {
        "listening" => "Listening — press any button on the panel now.".to_owned(),
        "hit" => match learn.device.as_deref() {
            Some(device) => format!("Seen, on {device}."),
            None => "Seen.".to_owned(),
        },
        "unavailable" => learn
            .error
            .clone()
            .unwrap_or_else(|| "the daemon's listener is not available".to_owned()),
        _ if !learn.ok => learn
            .error
            .clone()
            .unwrap_or_else(|| "the daemon's listener is not available".to_owned()),
        _ => {
            "Nothing is listening. Start the listener, then press a button on the panel.".to_owned()
        }
    }
}

/// One reason the wire form is not offered — the one that is actually true.
fn wire_blocked_line(session: &crate::control::SessionView, view: &ksx_api::SetupView) -> String {
    match (session.reachable, view.presets.is_empty()) {
        (true, false) => String::new(),
        (false, true) => "disabled — no daemon is running to take the write, and there is no \
                          preset on disk for a slot to point at. Start the daemon, and import \
                          or create a preset."
            .to_owned(),
        (false, false) => "disabled — wiring a slot is a daemon write, and no daemon is \
                           running. Start it and this control comes back."
            .to_owned(),
        (true, true) => "disabled — a slot points at a preset, and there is not one on disk \
                         yet. Import a configuration below, or run `ksx preset new`."
            .to_owned(),
    }
}

/// One reason the learner is not offered.
fn prove_blocked_line(
    session: &crate::control::SessionView,
    learn: &crate::control::LearnView,
) -> String {
    if !session.reachable {
        return "disabled — the listener lives in the daemon, and no daemon is running. \
                `ksx monitor` does the same job in a shell."
            .to_owned();
    }
    if learn.state == "unavailable" {
        let reason = learn
            .error
            .clone()
            .unwrap_or_else(|| "no reason given".to_owned());
        return format!(
            "disabled — the daemon is running, but its listener is not: {reason}. \
             `ksx monitor` does the same job in a shell."
        );
    }
    String::new()
}

/// What a slot write will do to the pads on THIS session.
fn wire_warning_line(session: &crate::control::SessionView) -> String {
    if session.running {
        "Wiring a slot REPLUGS the pads: every controller vanishes and comes back, and \
         anything mid-game sees it. Bindings do not — those swap in place."
            .to_owned()
    } else {
        "Nothing is running, so nothing replugs — the next start reads the new wiring. Wire \
         a slot while a session IS running and every controller vanishes and comes back."
            .to_owned()
    }
}

/// **Every `createShow` boolean on `/setup`, decided once, in Rust.**
///
/// Same rule and same reason as [`SetupLines`]: the seam injects these into the
/// SSR paint and `SetupIsland.ts` assigns them straight into its signals, so
/// the learner partition and the "is this readable" gate cannot be true on one
/// side of the seam and false on the other.
///
/// The two flash booleans are deliberately NOT here: a flash is one-shot action
/// feedback the client owns (it clears itself on a timer), so it is not a fact
/// about the machine and a poll must never rewrite it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupFlags {
    pub pill_running: bool,
    pub pill_idle: bool,
    pub pill_down: bool,
    pub no_daemon: bool,
    /// The machine provider REFUSED. Everything that would state a fact about
    /// this machine is gated off by it.
    pub setup_down: bool,
    /// The machine provider answered — the gate every inventory row, every
    /// checklist row and every "nothing here yet" sentence sits behind.
    pub setup_known: bool,
    pub first_run: bool,
    pub configured: bool,
    pub can_wire: bool,
    pub cannot_wire: bool,
    pub prove_down: bool,
    pub prove_listening: bool,
    pub prove_hit: bool,
    pub prove_idle: bool,
    pub has_boards: bool,
    pub no_boards: bool,
    pub has_slots: bool,
    pub no_slots: bool,
    pub has_notes: bool,
}

impl SetupFlags {
    pub fn of(
        setup: &SetupSnapshot,
        session: &crate::control::SessionView,
        learn: &crate::control::LearnView,
    ) -> Self {
        let view = &setup.view;
        let available = setup.available;

        // "Can this page write a slot?" is three facts now: a config we could
        // READ, a daemon to take the write, and a preset for the slot to point
        // at. A menu with no options and a live button is the shape that makes
        // a user think they did something.
        let wireable = available && session.reachable && !view.presets.is_empty();

        let listener_down = !session.reachable || learn.state == "unavailable";
        let listening = !listener_down && learn.state == "listening";
        let hit = !listener_down && learn.state == "hit";

        Self {
            pill_running: session.reachable && session.running,
            pill_idle: session.reachable && !session.running,
            pill_down: !session.reachable,
            no_daemon: !session.reachable,
            setup_down: !available,
            setup_known: available,
            first_run: available && !view.config_exists,
            configured: available && view.config_exists,
            can_wire: wireable,
            cannot_wire: !wireable,
            prove_down: listener_down,
            prove_listening: listening,
            prove_hit: hit,
            prove_idle: !listener_down && !listening && !hit,
            // EVERY one of these is `available &&`. Without it a refused read
            // renders "No board has a name yet" — a claim about a machine
            // nothing was read from.
            has_boards: available && !view.devices.is_empty(),
            no_boards: available && view.devices.is_empty(),
            has_slots: available && !view.slots.is_empty(),
            no_slots: available && view.slots.is_empty(),
            has_notes: available && !view.notes.is_empty(),
        }
    }
}

/// One checklist step as the row the page draws.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStepRowView {
    /// "1", "2", "3" — the position, as the badge text.
    pub badge: String,
    pub title: String,
    pub detail: String,
    /// `step done` | `step now` | `step later` — presentation of the BACKEND's
    /// state word, composed here so neither language re-derives it.
    pub cls: String,
}

/// A title-over-detail row (the inventory's boards and slots).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPairRowView {
    pub title: String,
    pub detail: String,
}

/// One `<option>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupOptionRowView {
    pub value: String,
    pub label: String,
}

/// One plain-text row (presets, profiles, notes).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupTextRowView {
    pub text: String,
}

/// **Every list row `/setup` draws, composed once, in Rust** — the same
/// docs/SURFACES.md §1 rule [`SetupLines`] and [`SetupFlags`] already follow,
/// applied to the row and label formatters.
///
/// These used to live twice: `render_setup.rs::list_values` composed
/// "Slot 3 — IPAC P1" for the SSR paint and `SetupIsland.ts` composed it
/// again for the two-second poll. Two copies of a SENTENCE drift silently
/// (the Profiles page's `ProfilesDerived` header tells the longer version of
/// this story); now both seams read these rows verbatim and format nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRows {
    pub steps: Vec<SetupStepRowView>,
    pub devices: Vec<SetupPairRowView>,
    pub slots: Vec<SetupPairRowView>,
    /// `1..=SetupView::max_slots` — the ceiling the backend serves, never a
    /// literal in either language.
    pub slot_options: Vec<SetupOptionRowView>,
    pub preset_options: Vec<SetupTextRowView>,
    pub profile_options: Vec<SetupTextRowView>,
    pub notes: Vec<SetupTextRowView>,
}

impl SetupRows {
    /// Compose every row for one read. The only implementation.
    pub fn of(setup: &SetupSnapshot) -> Self {
        let view = &setup.view;
        Self {
            steps: view
                .steps
                .iter()
                .enumerate()
                .map(|(i, step)| SetupStepRowView {
                    badge: (i + 1).to_string(),
                    title: step.title.clone(),
                    detail: step.detail.clone(),
                    cls: format!("step {}", step.state),
                })
                .collect(),
            devices: view
                .devices
                .iter()
                .map(|device| SetupPairRowView {
                    title: device.alias.clone(),
                    detail: format!("{} · {}", device.backend, device.id),
                })
                .collect(),
            slots: view
                .slots
                .iter()
                .map(|slot| SetupPairRowView {
                    title: format!("Slot {} — {}", slot.number, slot.preset),
                    detail: format!("{} · {} · {}", slot.device, slot.persona, slot.source),
                })
                .collect(),
            slot_options: (1..=view.max_slots)
                .map(|n| SetupOptionRowView {
                    value: n.to_string(),
                    label: format!("Slot {n}"),
                })
                .collect(),
            preset_options: view
                .presets
                .iter()
                .map(|name| SetupTextRowView { text: name.clone() })
                .collect(),
            profile_options: view
                .profiles
                .iter()
                .map(|title| SetupTextRowView {
                    text: title.clone(),
                })
                .collect(),
            notes: view
                .notes
                .iter()
                .map(|note| SetupTextRowView { text: note.clone() })
                .collect(),
        }
    }
}

/// What `GET /api/setup` serves AND what the setup island's props carry — the
/// same one-struct-one-serializer rule as [`StatusPayload`], parity pinned in
/// `render_setup.rs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPayload {
    pub setup: SetupSnapshot,
    pub session: crate::control::SessionView,
    /// Where the daemon's learner stands. This is step 3 — "press a button and
    /// watch it land" — and it is read on every page render, so the no-JS
    /// `<noscript>` refresh shows the press without any client code at all.
    pub learn: crate::control::LearnView,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/setup` — a poll is not an action.
    pub flash: Option<String>,
    /// The page's sentences, composed from the three above. Derived, never
    /// authored: [`SetupPayload::composed`] is what fills it.
    #[serde(default)]
    pub lines: SetupLines,
    /// The page's `createShow` booleans, decided from the three above. Same
    /// rule.
    #[serde(default)]
    pub flags: SetupFlags,
    /// The page's list rows, composed from [`Self::setup`]. Same rule again —
    /// see [`SetupRows`].
    #[serde(default)]
    pub rows: SetupRows,
}

impl SetupPayload {
    /// Recompose [`lines`](Self::lines), [`flags`](Self::flags) and
    /// [`rows`](Self::rows) from this payload's own facts.
    ///
    /// Called on the way OUT — by the render seam and by `/api/setup` — rather
    /// than at construction, so a payload assembled field by field (every test
    /// does, and so does the collector) can never serve sentences that
    /// contradict the facts sitting beside them.
    #[must_use]
    pub fn composed(mut self) -> Self {
        self.lines = SetupLines::of(&self.setup, &self.session, &self.learn);
        self.flags = SetupFlags::of(&self.setup, &self.session, &self.learn);
        self.rows = SetupRows::of(&self.setup);
        self
    }
}

// ───────────────────────────────────────────────────────────────────────────
// /start — the first run, moments 4 to 7 (docs/FIRST-RUN.md)
// ───────────────────────────────────────────────────────────────────────────

/// What `GET /api/start` serves AND what the `/start` island's props carry —
/// the same one-struct-one-serializer rule as [`StatusPayload`], parity pinned
/// in `render_start.rs`.
///
/// **Four reads, four failure modes, four fields.** They are kept apart for the
/// reason `docs/SURFACES.md` §1b gives: a daemon that is down and a machine
/// with no boards are opposite advice, and collapsing either into an empty
/// value is how a page ends up saying "you have staged nothing" when the truth
/// is "nothing answered". [`Self::staged`] carries its own `reachable` +
/// `error`; the other two carry theirs beside them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPayload {
    /// The staged setup, from `ControlSource::staged` — the DAEMON's memory,
    /// not a file. Its own `reachable`/`error` fields say when there is none.
    pub staged: ksx_api::StagedSetupView,
    /// The device enumeration, from `MachineSource::device_scan` — the same
    /// read `/devices` renders.
    pub scan: ksx_api::DeviceScanView,
    pub session: crate::control::SessionView,
    /// Empty when the scan answered. Otherwise the refusal, verbatim.
    #[serde(default)]
    pub unavailable: String,
    /// The presets ON DISK, from `MachineSource::presets`. Not what the stage
    /// holds — what a save would land next to, which is the only way this page
    /// can say "a preset of that name is already there" before the click.
    #[serde(default)]
    pub presets: Vec<ksx_api::PresetRow>,
    /// Empty when the preset read answered; otherwise the refusal. Separate
    /// from an empty list, because "no presets yet" is a first run and "I could
    /// not read the presets folder" is a broken install.
    #[serde(default)]
    pub presets_error: String,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/start` — a poll is not an action.
    pub flash: Option<String>,
    /// The page's sentences, composed from the four above. Derived, never
    /// authored — [`StartPayload::composed`] fills it.
    #[serde(default)]
    pub lines: StartLines,
    /// The page's `createShow` booleans, decided from the same four.
    #[serde(default)]
    pub flags: StartFlags,
    /// The page's list rows, same rule again.
    #[serde(default)]
    pub rows: StartRows,
}

impl StartPayload {
    /// Recompose [`lines`](Self::lines), [`flags`](Self::flags) and
    /// [`rows`](Self::rows) from this payload's own facts. Called on the way
    /// OUT, like [`SetupPayload::composed`], so a payload assembled field by
    /// field can never serve sentences that contradict the facts beside them.
    #[must_use]
    pub fn composed(mut self) -> Self {
        self.lines = StartLines::of(&self);
        self.flags = StartFlags::of(&self);
        self.rows = StartRows::of(&self);
        self
    }

    /// Is the device enumeration a reading of this machine at all?
    fn scan_read(&self) -> bool {
        self.unavailable.trim().is_empty()
    }
}

/// **Every sentence `/start` states as a fact, composed once, in Rust.**
///
/// Same rule and same reason as [`SetupLines`]: the SSR paint and the island's
/// poll show identical words because there is one implementation of them. The
/// island assigns these to signals and composes nothing.
///
/// What is deliberately NOT here: the split-or-freeze wording, the escape hatch
/// and the per-session scope. Those are `ksx_api::BlockingOption::roster()`,
/// `ESCAPE_HATCH_LINE` and `BLOCKING_SCOPE_LINE` — `docs/FIRST-RUN.md` §3 is a
/// question about what the CAPTURE THREAD does, so its words belong beside the
/// type that answers it and not on the one screen that currently asks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartLines {
    /// The keyboard step's heading line — chosen, or what to do about it.
    pub device_line: String,
    /// The chosen board's id, as SMALL PRINT. `FIRST-RUN.md` §5: the path
    /// belongs in small print for support, never as the identifier on screen.
    pub device_detail: String,
    /// `scan.boards_summary`, verbatim — the one line that distinguishes "no
    /// keyboard-capable board" from "nothing could be read".
    pub boards_line: String,
    /// The controller step's line: how many are staged and what that costs.
    pub controller_line: String,
    /// The XInput occupancy, from the SERVED numbers.
    pub xinput_line: String,
    /// Where the one question stands: the answer, or that it has not been
    /// asked. Never a pre-selected default — §3.
    pub blocking_line: String,
    /// What the mapper edits and where the staged preset stands relative to
    /// the presets on disk.
    pub preset_line: String,
    /// Ready to save or play, or ksx-core's own reason it is not.
    pub ready_line: String,
    /// What pressing Play actually does — moment 7's first sentence.
    pub play_line: String,
    /// Moment 7's one fact about the pad itself.
    pub guide_line: String,
    /// The daemon refusal that makes staging impossible, if any.
    pub stage_error: String,
    /// The scan refusal, if any.
    pub scan_error: String,
    /// The preset-read refusal, if any.
    pub presets_error: String,
}

impl StartLines {
    /// Compose every line for one payload. The only implementation.
    pub fn of(p: &StartPayload) -> Self {
        let staged = &p.staged;
        Self {
            device_line: match (&staged.device, staged.reachable) {
                (Some(device), _) => format!(
                    "Using {} — nothing has been claimed, plugged or written.",
                    device.label
                ),
                (None, true) => {
                    "Pick the keyboard you want to play with. Nothing happens when you pick it: \
                     it is remembered for this visit and written only if you save."
                        .to_owned()
                }
                // No daemon: the list below is still a real reading of the
                // machine, so the sentence is about the button, not the boards.
                (None, false) => "No keyboard can be chosen until a daemon answers — the staged \
                                  setup lives in it, not in this page."
                    .to_owned(),
            },
            device_detail: match &staged.device {
                Some(device) => format!(
                    "saved as [[device]] \"{}\" with id {} ({}) — {}",
                    device.alias,
                    device.selector,
                    device.rung,
                    if device.survives_replug {
                        "it still names this board after a move to another USB socket"
                    } else {
                        "it names THIS USB socket, so moving the board stops it matching"
                    }
                ),
                None => String::new(),
            },
            boards_line: p.scan.boards_summary.clone(),
            controller_line: controller_line(staged),
            xinput_line: format!(
                "{} of Windows' {} XInput slots would be used. Past that, PlayStation pads are \
                 how players {}+ exist — they are plain HID, so a game can read all of them.",
                staged.xinput_used,
                staged.max_xinput_slots,
                usize::from(staged.max_xinput_slots) + 1
            ),
            blocking_line: match &staged.blocking {
                Some(name) => match staged.blocking_options.iter().find(|o| &o.name == name) {
                    Some(chosen) => format!("Answered: {}.", chosen.title),
                    // A word the roster does not carry is a wire the surfaces
                    // disagree about, and saying so is better than rendering
                    // an unanswered screen over an answered setup.
                    None => format!(
                        "Answered with \"{name}\", which this build's list of answers does not \
                         contain — the setup and this page disagree, so treat the answer as \
                         unknown."
                    ),
                },
                None => "Not asked yet. There is no default here on purpose: a screen showing one \
                         option pre-selected has answered the question for you."
                    .to_owned(),
            },
            preset_line: preset_line(p),
            ready_line: match (&staged.not_ready, staged.reachable) {
                (_, false) => "Nothing can be saved or played until a daemon answers.".to_owned(),
                (Some(why), true) => why.clone(),
                (None, true) => "Ready. Save writes it, Play starts it, and either one works \
                                 without the other."
                    .to_owned(),
            },
            play_line: PLAY_LINE.to_owned(),
            guide_line: GUIDE_LINE.to_owned(),
            stage_error: staged.error.clone().unwrap_or_default(),
            scan_error: p.unavailable.trim().to_owned(),
            presets_error: p.presets_error.trim().to_owned(),
        }
    }
}

/// **What Play does**, stated before the button rather than after it.
///
/// The two halves are the ones a first-run user has no way to predict: a pad
/// appears on the ViGEm bus (so a game finds a controller that was not there a
/// second ago) and their keyboard changes behaviour (which, under Freeze, means
/// it stops typing). Both are reversible and the sentence says how — Stop, or
/// the escape latch, which is the same one §3's card carries.
const PLAY_LINE: &str =
    "Play plugs a virtual pad for each controller above and starts capturing the keyboard you \
     picked, so it becomes a controller. Stopping the session unplugs the pads and gives the \
     keyboard back — and so does LeftCtrl five times, from the keyboard itself.";

/// Moment 7's one fact about the pad that is not about ksx.
///
/// `ksx_core::pad::XButton::Guide` already exists and every persona publishes
/// it; what a first-run user does not know is that Windows answers it. Composed
/// here rather than in `ksx-api` because it is a sentence about this SCREEN's
/// last step, and no other surface has that step yet. The day the cabinet grows
/// one, it moves — the same way §3's wording already lives in `ksx-api`.
const GUIDE_LINE: &str =
    "Whatever you map to GUIDE opens the Xbox Game Bar, so you can start a game without going \
     back to a keyboard. It is an ordinary button on every persona — map it like any other.";

fn controller_line(staged: &ksx_api::StagedSetupView) -> String {
    match staged.slots.len() {
        0 => format!(
            "Pick what the keyboard should become. Nothing is plugged and nothing is written — \
             changing your mind costs a click. Up to {} controllers.",
            staged.max_slots
        ),
        1 => "1 controller staged. It does not exist yet: no pad is on the bus, no file has been \
              touched, and Remove leaves no trace."
            .to_owned(),
        n => format!(
            "{n} controllers staged. None of them exists yet: no pad is on the bus, no file has \
             been touched, and Remove leaves no trace."
        ),
    }
}

/// What the mapper step can honestly say, which depends on whether the presets
/// were readable at all.
///
/// The failed-read arm is the point: a preset name that "is not on disk" is a
/// claim about the presets folder, and when the read refused nothing is known
/// about it. Saying "this will create it" there is `SURFACES.md` §1b's bug.
fn preset_line(p: &StartPayload) -> String {
    const LEAD: &str = "Mapping happens in the mapper, and the mapper edits preset FILES. Save \
                        first: that writes one preset per controller and puts the slot in the \
                        list the mapper reads.";
    if !p.presets_error.trim().is_empty() {
        return format!(
            "{LEAD} What is already in the presets folder could not be read, so nothing here can \
             say whether saving would replace something."
        );
    }
    let clashes: Vec<&str> = p
        .staged
        .slots
        .iter()
        .filter(|slot| {
            p.presets
                .iter()
                .any(|row| row.name.eq_ignore_ascii_case(&slot.preset))
        })
        .map(|slot| slot.preset.as_str())
        .collect();
    if clashes.is_empty() {
        return format!("{LEAD} None of the names below is on disk yet, so saving creates them.");
    }
    format!(
        "{LEAD} {} already exists on disk — saving REPLACES it, keeping a timestamped copy the \
         mapper's \"Restore backup\" can put back.",
        clashes.join(", ")
    )
}

/// **Every `createShow` boolean on `/start`, decided once, in Rust.**
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartFlags {
    pub pill_running: bool,
    pub pill_idle: bool,
    pub pill_down: bool,
    /// The staged setup could not be reached — every verb on the page is inert
    /// and the banner says why.
    pub stage_down: bool,
    /// The device scan refused. NOT the same as an empty machine.
    pub scan_down: bool,
    /// The preset read refused.
    pub presets_down: bool,
    /// A keyboard is staged.
    pub has_device: bool,
    /// Boards that can be picked.
    pub has_boards: bool,
    /// **The enumeration ANSWERED and found no keyboard-capable board.** The
    /// only flag that licenses the "there is nothing here" paragraph; false
    /// whenever the list is empty because nothing could be read.
    pub no_boards: bool,
    /// Boards with no keyboard interface, listed so "why is my device not
    /// here" has an answer.
    pub has_other: bool,
    pub has_notes: bool,
    /// Controllers are staged.
    pub has_slots: bool,
    /// A controller can still be added (a free slot AND a persona to put in
    /// it AND a keyboard to drive it).
    pub can_add: bool,
    /// Every slot is taken — the ceiling, said before the button vanishes.
    pub slots_full: bool,
    /// Personas this build cannot plug, listed with the reason.
    pub has_gaps: bool,
    /// §3 has been answered.
    pub blocking_answered: bool,
    /// The setup is complete enough to save or play.
    pub ready: bool,
    pub not_ready: bool,
    /// Anything at all is staged, so "Start over" means something.
    pub can_discard: bool,
    /// A session is already running — starting a staged one replaces it, which
    /// the page says before the click.
    pub session_live: bool,
    pub flash_ok: bool,
    pub flash_error: bool,
}

impl StartFlags {
    /// Decide every branch for one payload. The only implementation.
    pub fn of(p: &StartPayload) -> Self {
        let staged = &p.staged;
        let session = &p.session;
        let scan_read = p.scan_read();
        let flash = p.flash.as_deref().unwrap_or_default().trim();
        let flash_error = flash.starts_with("error");
        Self {
            pill_running: session.reachable && session.running,
            pill_idle: session.reachable && !session.running,
            pill_down: !session.reachable,
            stage_down: !staged.reachable,
            scan_down: !scan_read,
            presets_down: !p.presets_error.trim().is_empty(),
            has_device: staged.device.is_some(),
            has_boards: scan_read && p.scan.pickable_boards > 0,
            // `no_pickable_board_found` and nothing else. `boards.is_empty()`
            // is the version that tells a cabinet with four boards plugged in
            // that it has none, on the one read where that is most wrong.
            no_boards: scan_read && p.scan.no_pickable_board_found,
            has_other: scan_read && p.scan.other_boards > 0,
            has_notes: !p.scan.notes.is_empty(),
            has_slots: !staged.slots.is_empty(),
            can_add: staged.reachable
                && staged.device.is_some()
                && staged.next_slot.is_some()
                && staged.personas.iter().any(|p| p.can_plug),
            slots_full: staged.reachable && staged.device.is_some() && staged.next_slot.is_none(),
            has_gaps: staged.personas.iter().any(|p| !p.can_plug),
            blocking_answered: staged.blocking.is_some(),
            ready: staged.reachable && staged.ready,
            not_ready: !staged.reachable || !staged.ready,
            can_discard: staged.reachable && !staged.empty,
            session_live: session.reachable && session.running,
            flash_ok: !flash.is_empty() && !flash_error,
            flash_error,
        }
    }
}

/// One board a first-run user could pick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartBoardRow {
    /// **The identifier on screen** — the vendor table's name (`FIRST-RUN.md`
    /// §5).
    pub name: String,
    /// `USB` | `Bluetooth`, as a human reads it.
    pub transport: String,
    /// What can reach it, served by `DeviceScanView::read` — the sentence that
    /// says a Bluetooth keyboard can be split but never WinUSB-claimed.
    pub backends: String,
    /// Its keyboard interface's verdict.
    pub verdict: String,
    /// The honest caveat when nothing on it DECLARES itself a keyboard, else
    /// empty. Rendered on every row and hidden when empty — a `createShow`
    /// inside a `createList` is not a shape this compiler emits.
    pub caveat: String,
    pub caveat_cls: String,
    /// "it is present and cannot type right now", when that applies.
    pub cannot_type: String,
    pub cannot_type_cls: String,
    /// SMALL PRINT: the Windows instance path, for a support conversation.
    /// Never the identifier — §5.
    pub path: String,
    /// What the form posts: the SERVED `DeviceSelector`, and the alias a pick
    /// would write. Neither is derived here and neither is ever typed.
    pub selector: String,
    pub alias: String,
    /// Is this the board already staged?
    pub chosen_cls: String,
    pub button: String,
}

/// One board that cannot be picked at all, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOtherRow {
    pub name: String,
    pub transport: String,
    pub reason: String,
    pub backends: String,
}

/// One staged controller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSlotRow {
    /// The form value.
    pub number: String,
    pub title: String,
    /// **Both halves of moment 5 in one line.** `FIRST-RUN.md` §1 says the
    /// controller "appears **ready**", and §2 says nothing has been plugged,
    /// claimed or written — and a row that said only the second reads as a
    /// half-finished thing rather than a decision that has been made.
    pub state: String,
    pub persona: String,
    /// Whether it occupies one of Windows' four XInput slots, as a sentence.
    pub xinput: String,
    /// The preset it binds, and how many controls that is — including zero,
    /// which is a real answer a page should say before a game does.
    pub preset: String,
    pub bindings: String,
}

/// One `<option>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptionRow {
    pub value: String,
    pub label: String,
}

/// One persona this build cannot plug, with `PadBackend::gap()`'s own sentence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartGapRow {
    pub label: String,
    pub gap: String,
    pub instead: String,
}

/// One answer to §3's question.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartBlockingRow {
    pub name: String,
    pub title: String,
    pub detail: String,
    /// Marks the answer that was actually given. Empty otherwise — no option
    /// is ever pre-marked, which is the whole point of `blocking` being
    /// optional.
    pub chosen_cls: String,
    pub button: String,
}

/// One plain-text row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTextRow {
    pub text: String,
}

/// **Every list row `/start` draws, composed once, in Rust.**
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRows {
    pub boards: Vec<StartBoardRow>,
    pub other: Vec<StartOtherRow>,
    pub notes: Vec<StartTextRow>,
    pub slots: Vec<StartSlotRow>,
    /// The personas this build CAN plug, in `Persona::ALL` order. Nothing here
    /// is spelled in TypeScript: `docs/SURFACES.md` §10 already settled that
    /// the roster is served with a `can_plug` flag per entry.
    pub personas: Vec<StartOptionRow>,
    /// The ones it cannot, listed rather than hidden — a menu that silently
    /// drops three of eight choices teaches a user the product has five.
    pub gaps: Vec<StartGapRow>,
    pub blocking: Vec<StartBlockingRow>,
}

impl StartRows {
    /// Compose every row for one payload. The only implementation.
    pub fn of(p: &StartPayload) -> Self {
        let staged = &p.staged;
        let chosen = staged.device.as_ref().map(|d| d.selector.as_str());
        Self {
            boards: p
                .scan
                .boards
                .iter()
                // `pickable`, never `keyboard.is_some()`: the partition is
                // `DeviceScanView::read`'s one decision and re-deriving it here
                // is how a seam and its island came to disagree about a count.
                .filter(|b| b.pickable)
                .map(|b| {
                    let selector = b.selector.clone().unwrap_or_default();
                    let is_chosen = !selector.is_empty() && chosen == Some(selector.as_str());
                    StartBoardRow {
                        name: b.name.clone(),
                        transport: b.transport_label.clone(),
                        backends: b.backends.clone(),
                        verdict: b.keyboard_verdict.clone(),
                        caveat: b.caveat.clone(),
                        caveat_cls: hidden_when_empty(&b.caveat, "dv-warn"),
                        cannot_type: b.cannot_type_line.clone(),
                        cannot_type_cls: hidden_when_empty(&b.cannot_type_line, "dv-warn"),
                        path: b.keyboard.clone().unwrap_or_default(),
                        selector,
                        alias: b.alias_hint.clone(),
                        chosen_cls: if is_chosen {
                            "pill pill-ok".to_owned()
                        } else {
                            "pill pill-none".to_owned()
                        },
                        button: if is_chosen {
                            "Chosen — pick it again".to_owned()
                        } else {
                            "Use this keyboard".to_owned()
                        },
                    }
                })
                .collect(),
            other: p
                .scan
                .boards
                .iter()
                .filter(|b| !b.pickable)
                .map(|b| StartOtherRow {
                    name: b.name.clone(),
                    transport: b.transport_label.clone(),
                    reason: b.keyboard_verdict.clone(),
                    backends: b.backends.clone(),
                })
                .collect(),
            notes: p
                .scan
                .notes
                .iter()
                .map(|note| StartTextRow { text: note.clone() })
                .collect(),
            slots: staged
                .slots
                .iter()
                .map(|slot| StartSlotRow {
                    number: slot.number.to_string(),
                    title: format!("Player {}", slot.number),
                    state: "ready — it will exist the moment you press Play".to_owned(),
                    persona: slot.persona_label.clone(),
                    xinput: if slot.is_xinput {
                        "uses an XInput slot".to_owned()
                    } else {
                        "plain HID — past the XInput four".to_owned()
                    },
                    preset: slot.preset.clone(),
                    bindings: match slot.bindings {
                        0 => "nothing mapped yet — this pad would plug and do nothing".to_owned(),
                        1 => "1 control bound".to_owned(),
                        n => format!("{n} controls bound"),
                    },
                })
                .collect(),
            personas: staged
                .personas
                .iter()
                .filter(|p| p.can_plug)
                .map(|p| StartOptionRow {
                    value: p.name.clone(),
                    label: p.label.clone(),
                })
                .collect(),
            gaps: staged
                .personas
                .iter()
                .filter(|p| !p.can_plug)
                .map(|p| StartGapRow {
                    label: p.label.clone(),
                    // `PadBackend::gap()`'s own sentence. A surface that
                    // paraphrased it into "install HIDMaestro" would be
                    // promising a fix that does not exist for two of the three.
                    gap: p.gap.clone().unwrap_or_default(),
                    instead: format!("Use {} instead.", p.instead),
                })
                .collect(),
            blocking: staged
                .blocking_options
                .iter()
                .map(|option| {
                    let is_chosen = staged.blocking.as_deref() == Some(option.name.as_str());
                    StartBlockingRow {
                        name: option.name.clone(),
                        title: option.title.clone(),
                        detail: option.detail.clone(),
                        chosen_cls: if is_chosen {
                            "pill pill-ok".to_owned()
                        } else {
                            "pill pill-none".to_owned()
                        },
                        button: if is_chosen {
                            "This is the answer".to_owned()
                        } else {
                            option.title.clone()
                        },
                    }
                })
                .collect(),
        }
    }
}

/// A line that is rendered on every row and HIDDEN when it has nothing to say
/// — `render_devices.rs`'s `optional_line`, and the same constraint: a
/// `createShow` inside a `createList` is not a shape this compiler emits.
///
/// The `dv-*` classes are `/devices`'s and are reused deliberately: this page
/// draws the same KIND of thing — a list of boards, each a stack of facts with
/// optional warnings — and a second set of class names for it would be a second
/// place to keep the amber plate and the hide rule in step.
fn hidden_when_empty(text: &str, class: &str) -> String {
    if text.trim().is_empty() {
        format!("{class} dv-hide")
    } else {
        class.to_owned()
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

    /// The same rule for the setup envelope: `SetupIsland.ts` reads these
    /// names, and the poller overwrites the signals the SSR paint seeded, so a
    /// rename on either side has to be a test failure rather than a page that
    /// flickers back to its defaults every two seconds.
    #[test]
    fn the_setup_payload_envelope_names_are_stable() {
        let payload = SetupPayload {
            setup: SetupSnapshot::ready(ksx_api::SetupView {
                config_root: "C:\\cfg\\ksx".into(),
                config_exists: true,
                ..ksx_api::SetupView::default()
            }),
            session: crate::control::SessionView::default(),
            learn: crate::control::LearnView::unavailable("no daemon"),
            flash: None,
            ..SetupPayload::default()
        }
        .composed();
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            v.pointer("/setup/available"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/setup/view/config_root"),
            Some(&serde_json::json!("C:\\cfg\\ksx"))
        );
        assert_eq!(
            v.pointer("/setup/view/config_exists"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/setup/view/steps"),
            Some(&serde_json::json!([])),
            "steps must always be an array — the page renders a list, not a maybe"
        );
        assert_eq!(
            v.pointer("/learn/state"),
            Some(&serde_json::json!("unavailable"))
        );
        assert_eq!(v.pointer("/flash"), Some(&serde_json::json!(null)));
        // The two DERIVED halves are part of the envelope too: `SetupIsland.ts`
        // reads `lines.*` and `flags.*` by these exact names and renders them
        // without deriving anything, so a rename here is a page that shows its
        // compile-time placeholders for ever.
        assert_eq!(
            v.pointer("/lines/config"),
            Some(&serde_json::json!(
                "Configured — 0 board(s), 0 slot(s), 0 preset(s)."
            ))
        );
        assert_eq!(
            v.pointer("/lines/wire_blocked"),
            Some(&serde_json::json!(
                "disabled — no daemon is running to take the write, and there is no preset \
                 on disk for a slot to point at. Start the daemon, and import or create a \
                 preset."
            ))
        );
        assert_eq!(
            v.pointer("/flags/setup_known"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/flags/no_boards"),
            Some(&serde_json::json!(true))
        );

        // A refused provider still produces the whole envelope, with the reason
        // in it: the page must be able to SAY why it has nothing.
        let down = SetupPayload {
            setup: SetupSnapshot::unavailable("no machine provider on this surface"),
            ..SetupPayload::default()
        };
        let v = serde_json::to_value(&down).unwrap();
        assert_eq!(
            v.pointer("/setup/available"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            v.pointer("/setup/source"),
            Some(&serde_json::json!("no machine provider on this surface"))
        );
    }

    fn live_session() -> crate::control::SessionView {
        crate::control::SessionView {
            reachable: true,
            running: true,
            line: "running — 4 pad(s)".into(),
            profile: None,
        }
    }

    fn idle_learner() -> crate::control::LearnView {
        crate::control::LearnView {
            ok: true,
            state: "idle".into(),
            remaining_ms: None,
            device: None,
            key: None,
            error: None,
        }
    }

    /// **"I could not read this" and "there is nothing here" are different
    /// sentences.**
    ///
    /// The signature bug of this project, in the config seam: a session once
    /// reported success while the arcade panel was dead, because a WinUSB board
    /// had fallen back to Interception. A page that renders a refused read as
    /// an empty machine makes the same trade — it converts "unknown" into a
    /// confident "nothing", and the user acts on it (they import, or they go
    /// looking for the board they were told is not named).
    ///
    /// This fails against the shipped version, where only `config` carried the
    /// `available` guard: there, every other line below was byte-identical to
    /// the empty-but-successfully-read machine's, and `no_boards` / `no_slots`
    /// were both true with nothing read.
    #[test]
    fn a_refused_read_is_never_rendered_as_an_empty_machine() {
        let session = live_session();
        let learn = idle_learner();

        let refused = SetupSnapshot::unavailable("no machine provider on this surface");
        let refused_lines = SetupLines::of(&refused, &session, &learn);
        let refused_flags = SetupFlags::of(&refused, &session, &learn);

        // A machine that WAS read and really holds nothing. Every sentence
        // below is the honest version of "there is nothing here".
        let empty = SetupSnapshot::ready(ksx_api::SetupView::default());
        let empty_lines = SetupLines::of(&empty, &session, &learn);
        let empty_flags = SetupFlags::of(&empty, &session, &learn);

        for (what, refused_line, empty_line) in [
            ("config", &refused_lines.config, &empty_lines.config),
            ("boards", &refused_lines.boards, &empty_lines.boards),
            ("slots", &refused_lines.slots, &empty_lines.slots),
            ("library", &refused_lines.library, &empty_lines.library),
            ("export", &refused_lines.export, &empty_lines.export),
            (
                "wire_blocked",
                &refused_lines.wire_blocked,
                &empty_lines.wire_blocked,
            ),
        ] {
            assert_ne!(
                refused_line, empty_line,
                "the {what} line says the same thing whether the read failed or the \
                 machine is empty: {refused_line:?}"
            );
        }

        // …and not one flag that would render an "it is empty" sentence.
        assert!(refused_flags.setup_down && !refused_flags.setup_known);
        for (what, on) in [
            ("no_boards", refused_flags.no_boards),
            ("no_slots", refused_flags.no_slots),
            ("has_boards", refused_flags.has_boards),
            ("has_slots", refused_flags.has_slots),
            ("has_notes", refused_flags.has_notes),
            ("first_run", refused_flags.first_run),
            ("configured", refused_flags.configured),
            // A daemon is reachable here: only the unreadable config stops it.
            ("can_wire", refused_flags.can_wire),
        ] {
            assert!(
                !on,
                "a refused read lit {what}, which renders a claim about a machine \
                 nothing was read from"
            );
        }
        // The empty-but-read machine still says all of those, loudly — the
        // guard must not have flattened both states into silence.
        assert!(empty_flags.setup_known && empty_flags.no_boards && empty_flags.no_slots);
        assert!(empty_lines
            .config
            .contains("no configuration on this machine"));
    }

    /// A disabled control names the reason that is TRUE, not the union of every
    /// reason it could have been.
    ///
    /// Fails against the shipped version, which rendered one static sentence
    /// ("Start the daemon, and import or create a preset first") for both
    /// single-cause states — telling a user with a live daemon to start it.
    #[test]
    fn a_disabled_control_names_the_cause_that_actually_fired() {
        let learn = idle_learner();
        let with_presets = SetupSnapshot::ready(ksx_api::SetupView {
            presets: vec!["IPAC P1".into()],
            ..ksx_api::SetupView::default()
        });
        let no_presets = SetupSnapshot::ready(ksx_api::SetupView::default());
        let up = live_session();
        let down = crate::control::SessionView::unreachable("no daemon");

        // Daemon UP, no preset: it must not tell them to start the daemon.
        let a = SetupLines::of(&no_presets, &up, &learn).wire_blocked;
        assert!(a.contains("preset"), "{a}");
        assert!(
            !a.to_lowercase().contains("start the daemon"),
            "told a user with a running daemon to start it: {a}"
        );

        // Daemon DOWN, presets on disk: it must not tell them to make a preset.
        let b = SetupLines::of(&with_presets, &down, &learn).wire_blocked;
        assert!(b.contains("daemon"), "{b}");
        assert!(
            !b.contains("preset new") && !b.contains("not one on disk"),
            "told a user with presets on disk to go and create one: {b}"
        );
        assert_ne!(a, b, "the two causes must not share one sentence");

        // Both wrong: one sentence, both facts.
        let both = SetupLines::of(&no_presets, &down, &learn).wire_blocked;
        assert!(both.contains("daemon") && both.contains("preset"), "{both}");

        // The learner's two down-states are likewise distinct — a reachable
        // daemon with a dead listener must not be told there is no daemon.
        let listener_dead = crate::control::LearnView::unavailable("no listener in this build");
        let dead_listener_line = SetupLines::of(&with_presets, &up, &listener_dead).prove_blocked;
        assert!(
            dead_listener_line.contains("the daemon is running"),
            "a live daemon with a dead listener was told there is no daemon: \
             {dead_listener_line}"
        );
        let no_daemon_line = SetupLines::of(&with_presets, &down, &listener_dead).prove_blocked;
        assert_ne!(dead_listener_line, no_daemon_line);
        // …and a working listener says nothing at all.
        assert_eq!(SetupLines::of(&with_presets, &up, &learn).prove_blocked, "");
    }

    /// The pad-bounce warning is about the session there IS.
    ///
    /// Fails against the shipped version, whose warning was unconditional: the
    /// wire form is offered whenever the daemon is REACHABLE, so an idle daemon
    /// got "every controller vanishes and comes back" for a write that would
    /// replug nothing.
    #[test]
    fn the_pad_bounce_warning_follows_the_running_session() {
        let learn = idle_learner();
        let view = SetupSnapshot::ready(ksx_api::SetupView::default());
        let idle = crate::control::SessionView {
            running: false,
            ..live_session()
        };

        let running_line = SetupLines::of(&view, &live_session(), &learn).wire_warning;
        assert!(running_line.contains("REPLUGS the pads"), "{running_line}");

        let idle_line = SetupLines::of(&view, &idle, &learn).wire_warning;
        assert!(
            !idle_line.contains("REPLUGS the pads"),
            "an idle daemon was told its controllers would vanish: {idle_line}"
        );
        assert!(idle_line.contains("Nothing is running"), "{idle_line}");
    }

    /// The composed halves are DERIVED: mutate a fact and the sentence beside
    /// it changes with it. A payload that cached its lines at construction
    /// would serve the old ones here.
    #[test]
    fn composing_a_payload_re_derives_its_lines_from_its_facts() {
        let payload = SetupPayload {
            setup: SetupSnapshot::ready(ksx_api::SetupView::default()),
            session: live_session(),
            learn: idle_learner(),
            flash: None,
            ..SetupPayload::default()
        }
        .composed();
        assert!(payload.lines.config.contains("no configuration"));
        assert!(payload.flags.setup_known);

        let mut refused = payload.clone();
        refused.setup = SetupSnapshot::unavailable("no machine provider");
        let refused = refused.composed();
        assert_eq!(refused.lines.config, UNREADABLE);
        assert!(refused.flags.setup_down && !refused.flags.no_boards);

        // Every field of both derived halves really moved.
        assert_ne!(payload.lines, refused.lines);
        assert_ne!(payload.flags, refused.flags);
    }

    /// The ROW sentences are composed here and nowhere else. This pins the
    /// exact strings both seams (render_setup.rs's SSR injection and
    /// SetupIsland.ts's poll) now read verbatim — the formatters they used to
    /// each own are gone, so this is the only place a row wording can change.
    #[test]
    fn the_setup_rows_are_composed_once_from_the_view() {
        let view = ksx_api::SetupView {
            devices: vec![ksx_api::SetupDeviceRow {
                alias: "P1 board".to_owned(),
                id: "usb:d209:0430:00".to_owned(),
                backend: "interception".to_owned(),
            }],
            slots: vec![ksx_api::SetupSlotRow {
                number: 3,
                device: "P1 board".to_owned(),
                preset: "IPAC P1".to_owned(),
                persona: "Xbox 360 pad".to_owned(),
                source: "config.toml".to_owned(),
            }],
            presets: vec!["IPAC P1".to_owned()],
            profiles: vec!["Street Fighter".to_owned()],
            steps: vec![ksx_api::SetupStep {
                id: ksx_api::setup_steps::SLOT.to_owned(),
                title: "Wire a slot".to_owned(),
                detail: "One slot is wired.".to_owned(),
                state: ksx_api::setup_states::NOW.to_owned(),
            }],
            notes: vec!["a note".to_owned()],
            ..ksx_api::SetupView::default()
        };
        let rows = SetupRows::of(&SetupSnapshot::ready(view));

        assert_eq!(rows.steps[0].badge, "1");
        assert_eq!(rows.steps[0].cls, "step now");
        assert_eq!(rows.devices[0].title, "P1 board");
        assert_eq!(rows.devices[0].detail, "interception · usb:d209:0430:00");
        assert_eq!(rows.slots[0].title, "Slot 3 — IPAC P1");
        assert_eq!(
            rows.slots[0].detail,
            "P1 board · Xbox 360 pad · config.toml"
        );
        assert_eq!(rows.preset_options[0].text, "IPAC P1");
        assert_eq!(rows.profile_options[0].text, "Street Fighter");
        assert_eq!(rows.notes[0].text, "a note");

        // The menu is 1..=the ceiling the BACKEND serves — never a literal in
        // a view layer (the shipped page held `SLOT_CHOICES = 8` in two
        // languages while `ksx_core::MAX_SLOTS` was 16).
        assert_eq!(rows.slot_options.len(), usize::from(ksx_core::MAX_SLOTS));
        assert_eq!(rows.slot_options[0].value, "1");
        assert_eq!(rows.slot_options[0].label, "Slot 1");
        let last = rows.slot_options.last().unwrap();
        assert_eq!(last.value, ksx_core::MAX_SLOTS.to_string());
    }
}
