//! Issue 565: sbxm writes a review's `Scope:` line itself, from the base and head commits it
//! gave the reviewer, replacing any the reviewer wrote, so a typo in commits sbxm already knows
//! can't keep the `sbxm/review` status from being posted.

use sbxm::task::review::{check_scope_end, with_scope};

const BASE: &str = "dd5587cb721bccab6d07e046ace75309c4f539c5";
const HEAD: &str = "8919ef53358e21138b676e611e5418e37d3c0e7d";

fn review(scope: &str) -> String {
    format!("Must-fix findings: 0\n\nRisk: low\n\n{scope}Issues: pending\n\n## Risk\n\n- docs\n")
}

fn sbxm_line() -> String {
    format!("Scope: `{BASE}..{HEAD}`")
}

fn scope_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("scope:"))
        .collect()
}

#[test]
fn a_base_one_character_too_long_is_replaced_and_the_scope_ends_at_the_head() {
    // PR #564's review: the base has an extra `d`, 41 characters.
    let typed = review(&format!("Scope: `d{BASE}..{HEAD}`\n"));
    assert!(check_scope_end(&typed, HEAD).is_err());

    let text = with_scope(&typed, BASE, HEAD);
    assert_eq!(scope_lines(&text), [sbxm_line()]);
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
    assert_eq!(text, review(&format!("{}\n", sbxm_line())));
}

#[test]
fn a_scope_ending_at_the_wrong_commit_is_replaced() {
    let typed = review(&format!("Scope: `{BASE}..{BASE}`\n"));
    let text = with_scope(&typed, BASE, HEAD);
    assert_eq!(scope_lines(&text), [sbxm_line()]);
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
}

#[test]
fn a_review_without_a_scope_line_gets_one_after_the_risk_line() {
    let text = with_scope(&review(""), BASE, HEAD);
    assert_eq!(scope_lines(&text), [sbxm_line()]);
    assert!(
        text.starts_with(&format!(
            "Must-fix findings: 0\n\nRisk: low\n\n{}\n",
            sbxm_line()
        )),
        "{text}"
    );
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
}

#[test]
fn a_review_without_scope_or_risk_lines_gets_one_after_the_first_line() {
    let text = with_scope("Must-fix findings: 0\n\nNothing.\n", BASE, HEAD);
    assert_eq!(
        text,
        format!("Must-fix findings: 0\n\n{}\n\nNothing.\n", sbxm_line())
    );
}

#[test]
fn two_scope_lines_leave_one_line_sbxms() {
    let typed = review(&format!(
        "Scope: `{BASE}..{BASE}`\nscope: `0000000..1111111`\n"
    ));
    let text = with_scope(&typed, BASE, HEAD);
    assert_eq!(scope_lines(&text), [sbxm_line()]);
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
}

#[test]
fn a_scope_line_quoted_in_a_code_block_is_left_alone() {
    let quoted = "```\nScope: `aaaaaaa..bbbbbbb`\n```\n";
    let typed = format!("{}\n## Must fix\n\n{quoted}", review(""));
    let text = with_scope(&typed, BASE, HEAD);
    assert!(text.contains(quoted), "{text}");
    assert_eq!(scope_lines(&text)[0], sbxm_line());
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
}

#[test]
fn windows_line_endings_are_kept() {
    let typed = "Must-fix findings: 0\r\n\r\nRisk: low\r\n\r\nScope: `x..y`\r\n\r\nNothing.\r\n";
    let text = with_scope(typed, BASE, HEAD);
    assert_eq!(
        text,
        format!(
            "Must-fix findings: 0\r\n\r\nRisk: low\r\n\r\n{}\r\n\r\nNothing.\r\n",
            sbxm_line()
        )
    );
}
