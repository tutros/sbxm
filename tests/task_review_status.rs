//! Issue 317: the `sbxm/review` commit status a review posts on the commit it covered. `success`
//! means a review covered that exact commit and found 0 must-fix, at any risk level and with or
//! without open questions; `failure` means it found must-fix findings. The description counts
//! both and names the risk, within GitHub's 140 characters.

use sbxm::github::StatusState;
use sbxm::task::review::{STATUS_CONTEXT, STATUS_DESCRIPTION_CAP, fit_status_description, status};
use sbxm::task::risk::Level;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

const ONE_QUESTION: &str = "Must-fix findings: 0\n\n\
    ## Questions\n\n\
    ### Q-1 - Should the hint name the repo?\n\n\
    **Where:** `src/a.rs:1`\n\
    **What happens:** it doesn't\n\
    **Why it matters:** decision 1\n\
    **Recommendation:** name it\n";

const ONE_MUST_FIX: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right one\n";

const SEE: &str = " -- see the sbxm review comment";

#[test]
fn no_must_fix_and_no_question_is_success() {
    let posted = status(0, CLEAN, Level::Low);

    assert_eq!(posted.state, StatusState::Success);
    assert_eq!(posted.context, "sbxm/review");
    assert_eq!(STATUS_CONTEXT, "sbxm/review");
    assert_eq!(
        posted.description,
        format!("0 must-fix, 0 questions, risk low{SEE}")
    );
}

#[test]
fn an_open_question_is_still_success_and_is_counted() {
    let posted = status(0, ONE_QUESTION, Level::Medium);

    assert_eq!(posted.state, StatusState::Success);
    assert_eq!(
        posted.description,
        format!("0 must-fix, 1 question, risk medium{SEE}")
    );
}

#[test]
fn a_must_fix_finding_is_failure_and_is_counted() {
    let posted = status(1, ONE_MUST_FIX, Level::Low);

    assert_eq!(posted.state, StatusState::Failure);
    assert_eq!(
        posted.description,
        format!("1 must-fix, 0 questions, risk low{SEE}")
    );
}

#[test]
fn the_risk_level_never_changes_the_state() {
    for level in [Level::Low, Level::Medium, Level::High, Level::Unknown] {
        let posted = status(0, CLEAN, level);
        assert_eq!(posted.state, StatusState::Success, "{level:?}");
        assert!(
            posted
                .description
                .starts_with(&format!("0 must-fix, 0 questions, risk {}", level.word())),
            "{}",
            posted.description
        );
    }
}

#[test]
fn a_description_at_the_cap_is_kept_whole() {
    let text = "é".repeat(STATUS_DESCRIPTION_CAP);

    assert_eq!(STATUS_DESCRIPTION_CAP, 140);
    assert_eq!(fit_status_description(&text), text);
}

#[test]
fn a_description_one_over_the_cap_is_cut_to_fit() {
    let text = "é".repeat(STATUS_DESCRIPTION_CAP + 1);

    let fitted = fit_status_description(&text);

    assert_eq!(fitted.chars().count(), STATUS_DESCRIPTION_CAP);
    assert!(text.starts_with(&fitted));
}

#[test]
fn the_largest_count_still_fits_with_the_pointer_to_the_comment() {
    let posted = status(u32::MAX, CLEAN, Level::Medium);
    assert!(posted.description.chars().count() <= STATUS_DESCRIPTION_CAP);
    assert_eq!(
        posted.description,
        format!("4294967295 must-fix, 0 questions, risk medium{SEE}")
    );
}
