//! Issue 565: sbxm writes a review's `Scope:` line itself, from the base and head commits it
//! gave the reviewer, replacing any the reviewer wrote, so a typo in commits sbxm already knows
//! can't keep the `sbxm/review` status from being posted.

use sbxm::github::StatusState;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::pipeline::{StatusInput, post_status};
use sbxm::task::review::{check_scope_end, with_scope};
use sbxm::task::risk::Level;

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
    // sbxm's line always goes right after the Risk line, set off by a blank line (review M-1).
    assert_eq!(text, review(&format!("{}\n\n", sbxm_line())));
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

// Review M-1: sbxm's line must be the first Scope line the parser reads, so it goes at the top
// even when a quoted range comes before the reviewer's own line.

const QUOTED_FIRST: &str = "Must-fix findings: 0\nRisk: low\n\nAn earlier review used this range:\n```\nScope: `1111111..2222222`\n```\n\nScope: `3333333..4444444`\nIssues: pending\n";

#[test]
fn a_quoted_scope_before_the_reviewers_own_is_kept_below_sbxms() {
    let text = with_scope(QUOTED_FIRST, BASE, HEAD);

    assert!(
        text.contains("```\nScope: `1111111..2222222`\n```\n"),
        "{text}"
    );
    assert!(!text.contains("3333333..4444444"), "{text}");
    assert!(
        text.starts_with(&format!(
            "Must-fix findings: 0\nRisk: low\n\n{}\n",
            sbxm_line()
        )),
        "{text}"
    );
    assert_eq!(check_scope_end(&text, HEAD), Ok(()));
}

#[test]
fn a_quoted_scope_before_the_reviewers_own_still_lets_the_status_be_posted() {
    let text = with_scope(QUOTED_FIRST, BASE, HEAD);
    let github = FakeGitHub::default();

    let warning = post_status(
        &github,
        "o/r",
        &StatusInput {
            commit: Some(HEAD),
            must_fix: 0,
            text: &text,
            risk: Level::Low,
        },
        "sbxm task review --pr 7 --force",
    );

    assert_eq!(warning, None);
    let calls = github.calls();
    let [GhCall::CommitStatus(_, sha, status)] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(sha, HEAD);
    assert_eq!(status.state, StatusState::Success);
}

#[test]
fn a_scope_line_before_the_must_fix_count_is_left_so_the_review_is_still_refused() {
    let typed = format!("Scope: `{BASE}..{HEAD}`\nMust-fix findings: 0\n\nNothing.\n");
    let text = with_scope(&typed, BASE, HEAD);
    assert!(text.starts_with("Scope: "), "{text}");
    assert_eq!(sbxm::task::review::must_fix_count(&text), None);
}
