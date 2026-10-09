//! Issue 123 (decision 176): the risk assessment of a task's change. The reviewer writes a
//! `Risk: low|medium|high` line plus a `## Risk` section (parsed like `Must-fix findings:`), and
//! sbxm raises the level to a minimum from path rules, but never lowers it.

use sbxm::task::review::{risk_level, risk_reasons, saved_risk_level, saved_risk_reasons};
use sbxm::task::risk::{Level, built_in_rules, floor, render_section};

#[test]
fn a_low_risk_line_parses() {
    assert_eq!(
        risk_level("Must-fix findings: 0\n\nRisk: low\n"),
        Some(Level::Low)
    );
}

#[test]
fn a_medium_risk_line_parses() {
    assert_eq!(
        risk_level("Must-fix findings: 1\n\nRisk: medium\n"),
        Some(Level::Medium)
    );
}

#[test]
fn a_high_risk_line_parses() {
    assert_eq!(
        risk_level("Must-fix findings: 1\n\nRisk: high\n"),
        Some(Level::High)
    );
}

#[test]
fn a_missing_risk_line_is_none() {
    assert_eq!(risk_level("Must-fix findings: 0\n\nNothing found.\n"), None);
}

#[test]
fn an_unparseable_risk_value_is_none() {
    assert_eq!(risk_level("Must-fix findings: 0\n\nRisk: critical\n"), None);
}

#[test]
fn the_reviewer_may_never_claim_unknown_itself() {
    assert_eq!(risk_level("Must-fix findings: 0\n\nRisk: unknown\n"), None);
}

#[test]
fn a_crlf_risk_line_still_parses() {
    assert_eq!(
        risk_level("Must-fix findings: 0\r\n\r\nRisk: medium\r\n"),
        Some(Level::Medium)
    );
}

/// Issue 123, M-1 of the issue-123 review: a `Risk:`-looking line quoted elsewhere in the body
/// (here, inside prose further down) must not be mistaken for the required header, which sits
/// right after the `Must-fix findings:` line.
#[test]
fn a_risk_line_quoted_later_in_the_body_is_not_the_header() {
    let review = "Must-fix findings: 0\n\n## Summary\n\nquoted output:\nRisk: high\n";
    assert_eq!(risk_level(review), None);
}

/// The blank separator line is optional: a reviewer who writes the risk line directly after the
/// must-fix line, with nothing between, still parses.
#[test]
fn a_risk_line_with_no_blank_separator_still_parses() {
    assert_eq!(
        risk_level("Must-fix findings: 0\nRisk: medium\n"),
        Some(Level::Medium)
    );
}

#[test]
fn risk_reasons_are_read_from_their_own_section() {
    let review = "Must-fix findings: 0\n\nRisk: medium\n\n## Risk\n\n\
        - a behavior change in one area\n- touches the config loader\n\n## Summary\n\nchecked it\n";
    assert_eq!(
        risk_reasons(review),
        ["a behavior change in one area", "touches the config loader"]
    );
}

#[test]
fn a_quoted_risk_section_in_code_gives_no_reasons() {
    let fenced = "Must-fix findings: 0\n\nRisk: low\n\n## Summary\n\n```markdown\n## Risk\n\n\
        - not an assessment\n```\n";
    assert_eq!(risk_reasons(fenced), Vec::<String>::new());
    let indented = "Must-fix findings: 0\n\nRisk: low\n\n## Summary\n\n    ## Risk\n    \
        - not an assessment\n";
    assert_eq!(risk_reasons(indented), Vec::<String>::new());
}

#[test]
fn only_the_first_real_risk_section_counts() {
    let review = "Must-fix findings: 0\n\nRisk: low\n\n## Risk\n\n- contained\n\n## Summary\n\n\
        ~~~\n## Risk\n- quoted\n~~~\n\n## Risk\n\n- a second section\n";
    assert_eq!(risk_reasons(review), ["contained"]);
}

#[test]
fn a_review_with_no_risk_section_has_no_reasons() {
    assert_eq!(
        risk_reasons("Must-fix findings: 0\n\nNothing found.\n"),
        Vec::<String>::new()
    );
}

#[test]
fn saved_variants_skip_the_reviewer_header() {
    let saved = "Reviewer: codex (gpt-5.6-sol)\n\nMust-fix findings: 0\n\nRisk: low\n\n## Risk\n- contained\n";
    assert_eq!(saved_risk_level(saved), Some(Level::Low));
    assert_eq!(saved_risk_reasons(saved), ["contained"]);
}

#[test]
fn a_ci_workflow_path_floors_to_medium() {
    let rules = built_in_rules();
    let (level, reasons) = floor(&[".github/workflows/ci.yml".to_owned()], &rules);
    assert_eq!(level, Level::Medium);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("ci.yml"), "{reasons:?}");
}

#[test]
fn a_lockfile_path_floors_to_medium() {
    let rules = built_in_rules();
    let (level, _) = floor(&["Cargo.lock".to_owned()], &rules);
    assert_eq!(level, Level::Medium);
}

#[test]
fn a_secret_named_path_floors_to_medium() {
    let rules = built_in_rules();
    let (level, _) = floor(&["config/.env.production".to_owned()], &rules);
    assert_eq!(level, Level::Medium);
}

#[test]
fn an_unrelated_path_does_not_floor() {
    let rules = built_in_rules();
    let (level, reasons) = floor(&["src/lib.rs".to_owned()], &rules);
    assert_eq!(level, Level::Unknown);
    assert!(reasons.is_empty());
}

#[test]
fn several_files_in_the_same_category_give_one_reason() {
    let rules = built_in_rules();
    let (level, reasons) = floor(
        &[
            ".github/workflows/ci.yml".to_owned(),
            ".github/workflows/release.yml".to_owned(),
        ],
        &rules,
    );
    assert_eq!(level, Level::Medium);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
}

#[test]
fn the_floor_never_lowers_a_higher_parsed_level() {
    // The reviewer's own level (high) combined with a medium floor stays high; this is the
    // caller's job (`Level::max`), exercised here as the contract `floor` must support.
    let rules = built_in_rules();
    let (floor_level, _) = floor(&["Cargo.lock".to_owned()], &rules);
    assert_eq!(Level::High.max(floor_level), Level::High);
}

#[test]
fn levels_order_low_to_high() {
    assert!(Level::Unknown < Level::Low);
    assert!(Level::Low < Level::Medium);
    assert!(Level::Medium < Level::High);
}

#[test]
fn render_section_shows_the_marker_level_and_reasons() {
    let text = render_section(Level::High, &["touches src/git.rs".to_owned()]);
    assert!(text.starts_with("## Risk: \u{1f534} high\n"), "{text}");
    assert!(text.contains("- touches src/git.rs"), "{text}");
}

#[test]
fn render_section_without_reasons_says_so() {
    let text = render_section(Level::Unknown, &[]);
    assert!(text.contains("no reasons given"), "{text}");
}
