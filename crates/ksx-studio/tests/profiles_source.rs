//! Authored-source contract for the Games island.
//!
//! Generated FMIR/assets are release build output and are intentionally not
//! rewritten by narrow backend work. These assertions pin the customer form
//! source itself so the next normal UI generation cannot silently omit the
//! optimistic revision or regress the layout select into free text.

const SOURCE: &str = include_str!("../../../studio-ui/src/ProfilesIsland.ts");

#[test]
fn update_and_delete_return_the_opaque_row_revision() {
    assert_eq!(
        SOURCE.matches(r#"name: "revision""#).count(),
        4,
        "both live/plain rows need one update and one delete revision field"
    );
    assert_eq!(SOURCE.matches("value: g.revision").count(), 4);
}

#[test]
fn games_actions_and_layout_edits_are_customer_facing() {
    assert!(SOURCE.contains(r#""Play game""#));
    assert!(SOURCE.contains(r#"action: "/profiles/stop""#));
    assert!(SOURCE.contains(r#""Stop playing""#));
    assert_eq!(
        SOURCE.matches("() => presetOptions()").count(),
        3,
        "both edit forms and the add form must use the served valid-layout choices"
    );
    assert_eq!(SOURCE.matches("selected: true").count(), 2);
    assert_eq!(SOURCE.matches("hidden: true").count(), 2);
    assert!(
        !SOURCE.contains("() => g.layout_options"),
        "Forma IR cannot connect a list nested over a row property to its array slot"
    );
    assert!(
        !SOURCE.contains(r#""Switch","#),
        "the customer action is immediate Play, not an unexplained Switch"
    );
    assert!(
        !SOURCE.contains(r#"{ class: "ptitle" }, t.id"#),
        "starter-layout ids are values/keys, not customer labels"
    );
    assert!(SOURCE.contains("This screen is temporarily unavailable. Reopen ksx and try again."));
}

#[test]
fn browser_flashes_are_allowlisted_before_they_reach_a_signal() {
    assert!(SOURCE.contains("export function safeProfileFlash"));
    assert!(SOURCE.contains("PROFILE_FLASH_ALLOWLIST.includes(candidate)"));
    assert!(SOURCE.contains("const line = safeProfileFlash(flash)"));
    assert!(SOURCE
        .contains("error: Saved Games could not finish that request. Reopen ksx and try again."));
}
