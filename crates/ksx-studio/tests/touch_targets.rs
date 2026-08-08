//! **The touch contract, enforced — and the cascade trap that made it a lie.**
//!
//! `docs/DESIGN-SYSTEM.md` §2 and §11 both promise that under
//! `@media (pointer: coarse)` the control vocabulary grows to a 40 px minimum
//! (36 px for the small tier), "because a cabinet screen is touched". Until
//! 2026-08-08 that promise was written as a list of component selectors, and
//! **in the rendered output it was false for every field on every screen**.
//!
//! The reason is pure cascade, not CSS exotica. The coarse block sat in §4.3,
//! and §4.4 — thirty lines BELOW it — re-declares
//!
//! ```css
//! select, input[type="text"], input[type="number"] { min-height: var(--ctl-h); }
//! ```
//!
//! Same specificity, later in the file, so the later rule wins and the media
//! query does nothing. Measured against the HTML `render_pads` /
//! `render_profiles` / `render_setup` / `render_devices` actually emit, in
//! Chromium with touch emulation on: /pads' three spawn selects, /profiles'
//! six new-profile fields, /setup's four selects and /devices' alias box all
//! rendered 36 px. Nobody had ever measured them, because reading the source
//! shows a `@media (pointer: coarse)` block that says 2.5rem and looks right —
//! the same "audit the OUTPUT, not the source" failure that
//! `render.rs::assert_complete_head` was written for.
//!
//! # What this file checks
//!
//! Two things, and neither of them restates the fix:
//!
//! 1. **[`no_coarse_declaration_is_cancelled_by_a_later_unconditional_rule`]**
//!    — the general form of the bug. A declaration inside `(pointer: coarse)`
//!    exists to be an *exception*; an unconditional rule that lands later in
//!    the file with an identical selector and a covering property silently
//!    cancels it, in a way no reader of either rule can see. This is a lint
//!    over the whole stylesheet, so it catches the next one too, wherever it
//!    is added. **It fails against the pre-fix studio.css on exactly the three
//!    field selectors that were dead**, and on nothing else.
//!
//! 2. **[`touch_grows_the_ladder_itself`]** — the ladder tokens are what the
//!    fix moves, and they are re-derived here rather than restated: 40 px is
//!    the number DESIGN-SYSTEM §2/§11 print, and a token that stops meeting it
//!    means the doc is lying again. Fails against the pre-fix file, which has
//!    no `:root` inside a coarse query at all.
//!
//! Neither test can see a browser, and neither pretends to: the geometry was
//! measured by hand, once, and what is pinned here is the pair of *structural*
//! properties that made the measurement come out wrong.

const CSS: &str = include_str!("../../../studio-ui/src/studio.css");

/// One style rule, in document order, with the at-rule preludes enclosing it.
#[derive(Debug)]
struct Rule {
    order: usize,
    /// `@media (...)` preludes from outermost in. Empty = unconditional.
    context: Vec<String>,
    /// One entry per comma-separated selector.
    selectors: Vec<String>,
    /// `(property, value)` in source order.
    decls: Vec<(String, String)>,
    line: usize,
}

/// Drop `/* … */`, keeping newlines so reported line numbers stay true.
fn strip_comments(css: &str) -> String {
    let bytes = css.as_bytes();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let end = css[i + 2..]
                .find("*/")
                .map_or(css.len(), |at| i + 2 + at + 2);
            for c in css[i..end].chars() {
                if c == '\n' {
                    out.push('\n');
                }
            }
            i = end;
        } else {
            let c = css[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// A deliberately small CSS reader: enough for this stylesheet, which is
/// prettier-formatted and contains no `{`/`}` inside a string or a `url()`.
/// It is not a parser for the language — it is a parser for THIS file, and it
/// panics rather than guessing if the shape it assumes stops holding.
fn rules(css: &str) -> Vec<Rule> {
    let src = strip_comments(css);
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut prelude = String::new();
    let mut order = 0usize;
    let mut line = 1usize;

    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                line += 1;
                prelude.push(' ');
            }
            '{' => {
                let head = prelude.trim().to_owned();
                prelude.clear();
                if head.starts_with('@') {
                    stack.push(head);
                    continue;
                }
                // A style rule: read its declarations to the matching `}`.
                let mut body = String::new();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '\n' {
                        line += 1;
                    }
                    if c == '{' {
                        depth += 1;
                    }
                    if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    body.push(c);
                }
                assert_eq!(depth, 0, "unbalanced braces near line {line} in studio.css");
                order += 1;
                out.push(Rule {
                    order,
                    context: stack.clone(),
                    selectors: head
                        .split(',')
                        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                        .filter(|s| !s.is_empty())
                        .collect(),
                    decls: body
                        .split(';')
                        .filter_map(|d| d.split_once(':'))
                        .map(|(p, v)| (p.trim().to_ascii_lowercase(), v.trim().to_owned()))
                        .filter(|(p, _)| !p.is_empty())
                        .collect(),
                    line,
                });
            }
            '}' => {
                stack.pop();
            }
            _ => prelude.push(c),
        }
    }
    out
}

/// Does `outer` decide the used value of `inner`? `padding` cancels a later
/// `padding-inline`; `min-height` only cancels `min-height`. Shorthands are
/// the half of this that is easy to get wrong by hand — the first attempt at
/// the `.maplink` fix set `padding-inline` inside the coarse query and lost to
/// `.maplink { padding: 0 var(--sp-2) }` forty lines below it, which measured
/// as "40 px tall and still 38.8 px wide".
fn covers(outer: &str, inner: &str) -> bool {
    if outer == inner {
        return true;
    }
    const SHORTHANDS: &[&str] = &[
        "padding",
        "margin",
        "border",
        "border-radius",
        "border-width",
        "border-color",
        "border-style",
        "inset",
        "gap",
        "flex",
        "font",
        "background",
        "overflow",
        "grid-template",
        "place-items",
        "place-content",
        "transition",
        "animation",
    ];
    SHORTHANDS.contains(&outer) && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'-'
}

fn is_coarse(context: &[String]) -> bool {
    context.iter().any(|c| c.contains("pointer: coarse"))
}

/// **The lint.** Fails against the studio.css of 2026-08-07, naming
/// `select`, `input[type="text"]` and `input[type="number"]` — the three
/// selectors whose `min-height: 2.5rem` was cancelled by §4.4's
/// `min-height: var(--ctl-h)` twenty-seven lines later.
#[test]
fn no_coarse_declaration_is_cancelled_by_a_later_unconditional_rule() {
    let rules = rules(CSS);
    let mut dead: Vec<String> = Vec::new();

    for rule in rules.iter().filter(|r| is_coarse(&r.context)) {
        for selector in &rule.selectors {
            for (prop, _) in &rule.decls {
                // A custom property is not in the cascade with the rules that
                // READ it — that is the whole reason the ladder lives on
                // `:root` — so it can only be cancelled by another `--` of the
                // same name, which this same loop already covers.
                for later in rules
                    .iter()
                    .filter(|l| l.order > rule.order && l.context.is_empty())
                    .filter(|l| l.selectors.iter().any(|s| s == selector))
                {
                    if let Some((cancel, _)) = later.decls.iter().find(|(p, _)| covers(p, prop)) {
                        dead.push(format!(
                            "line {}: `{selector} {{ {prop} }}` inside (pointer: coarse) is \
                             cancelled by `{selector} {{ {cancel} }}` at line {} — same \
                             specificity, later in the file, so the media query does nothing. \
                             Declare it AFTER that rule (see studio.css §4.4b).",
                            rule.line, later.line
                        ));
                    }
                }
            }
        }
    }

    assert!(
        dead.is_empty(),
        "{} touch-target declaration(s) are dead in the rendered page:\n  {}",
        dead.len(),
        dead.join("\n  ")
    );
}

/// The ladder is what grows, and it grows to the number the doc prints.
///
/// Fails against the pre-fix stylesheet, whose coarse query contained no
/// `:root` block at all — it listed components instead, which is what let the
/// fields fall out of the vocabulary without anything noticing.
#[test]
fn touch_grows_the_ladder_itself() {
    let ladder: Vec<(String, String)> = rules(CSS)
        .into_iter()
        .filter(|r| is_coarse(&r.context) && r.selectors.iter().any(|s| s == ":root"))
        .flat_map(|r| r.decls)
        .collect();

    let rem = |name: &str| -> f64 {
        let raw = ladder
            .iter()
            .find(|(p, _)| p == name)
            .unwrap_or_else(|| {
                panic!(
                    "(pointer: coarse) must redefine {name} on :root — DESIGN-SYSTEM §2 says the \
                     WHOLE vocabulary grows on touch, and moving the ladder token is the only way \
                     a component rule added later cannot silently opt out of it"
                )
            })
            .1
            .clone();
        raw.trim_end_matches("rem")
            .trim()
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("{name} is `{raw}`, which is not a rem step"))
    };

    // DESIGN-SYSTEM §2: "a 40 px minimum (36 px for the small tier)"; §11:
    // "Hit targets grow to 40 px under pointer: coarse".
    assert!(
        rem("--ctl-h") * 16.0 >= 40.0,
        "--ctl-h is {}px on touch; §2 and §11 both promise 40",
        rem("--ctl-h") * 16.0
    );
    assert!(
        rem("--ctl-h-sm") * 16.0 >= 36.0,
        "--ctl-h-sm is {}px on touch; §2 promises 36 for the small tier",
        rem("--ctl-h-sm") * 16.0
    );
}
