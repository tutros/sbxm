//! Issue 548 (decision 316(3), #460 guardrail 5): the veto window before a medium-risk PR is
//! armed. Two hours from the head's latest `sbxm/review` success; a `hold` label stops arming, a
//! `go` label arms at once, neither arms when the window ends. Nothing calls it before #550.

use sbxm::github::PostedStatus;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::window::{self, Window};

const REPO: &str = "o/r";
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// When the head's review status was posted.
const REVIEWED: u64 = 1_794_339_600;
const HOUR: u64 = 3_600;

fn status(context: &str, state: &str, at: u64) -> PostedStatus {
    PostedStatus {
        context: context.into(),
        state: state.into(),
        at,
    }
}

fn review(state: &str, at: u64) -> PostedStatus {
    status("sbxm/review", state, at)
}

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// The window of a PR passing every other rule, reviewed clean at [`REVIEWED`].
fn at(now: u64, names: &[&str]) -> Window {
    let start = window::start(&[review("success", REVIEWED)]);
    window::decide(&labels(names), start, &[], false, now)
}

// --- Window time ---

#[test]
fn one_second_before_two_hours_it_waits_until_the_windows_end() {
    assert_eq!(
        at(REVIEWED + 2 * HOUR - 1, &[]),
        Window::Waiting {
            until: REVIEWED + 2 * HOUR
        }
    );
}

#[test]
fn at_exactly_two_hours_the_window_is_over() {
    assert_eq!(at(REVIEWED + 2 * HOUR, &[]), Window::Over);
}

#[test]
fn one_second_after_two_hours_the_window_is_over() {
    assert_eq!(at(REVIEWED + 2 * HOUR + 1, &[]), Window::Over);
}

#[test]
fn a_head_without_a_review_status_is_not_eligible() {
    let start = window::start(&[status("ci", "success", REVIEWED)]);

    let decided = window::decide(&[], start, &[], false, REVIEWED + 3 * HOUR);

    assert!(
        matches!(&decided, Window::NotEligible(why) if why.contains("sbxm/review")),
        "{decided:?}"
    );
    assert!(!decided.arm_now());
}

#[test]
fn a_head_whose_latest_review_status_is_a_failure_is_not_eligible() {
    // Newest first, as GitHub lists them: a re-review that failed after a clean one.
    let start = window::start(&[
        review("failure", REVIEWED + HOUR),
        review("success", REVIEWED),
    ]);

    let decided = window::decide(&[], start, &[], false, REVIEWED + 4 * HOUR);

    assert!(
        matches!(&decided, Window::NotEligible(why) if why.contains("failure")),
        "{decided:?}"
    );
}

#[test]
fn of_two_success_statuses_the_later_one_starts_the_window() {
    let later = REVIEWED + HOUR;
    // Listed in either order, the later one counts.
    for statuses in [
        vec![review("success", later), review("success", REVIEWED)],
        vec![review("success", REVIEWED), review("success", later)],
    ] {
        let start = window::start(&statuses);

        assert_eq!(start, Ok(later));
        assert_eq!(
            window::decide(&[], start, &[], false, REVIEWED + 2 * HOUR),
            Window::Waiting {
                until: later + 2 * HOUR
            }
        );
    }
}

// --- Labels ---

#[test]
fn without_labels_it_is_decided_by_time() {
    assert_eq!(
        at(REVIEWED, &["size:M"]),
        Window::Waiting {
            until: REVIEWED + 2 * HOUR
        }
    );
    assert_eq!(at(REVIEWED + 2 * HOUR, &["size:M"]), Window::Over);
}

#[test]
fn a_hold_label_holds_it() {
    let decided = at(REVIEWED + 3 * HOUR, &["hold"]);

    assert_eq!(decided, Window::Held);
    assert!(!decided.arm_now());
    assert_eq!(decided.how(), Some("held"));
}

#[test]
fn a_go_label_arms_it_at_once_even_at_zero_minutes() {
    let decided = at(REVIEWED, &["go"]);

    assert_eq!(decided, Window::Go);
    assert!(decided.arm_now());
    assert_eq!(decided.how(), Some("go"));
}

#[test]
fn the_windows_end_says_so() {
    let decided = at(REVIEWED + 2 * HOUR, &[]);

    assert!(decided.arm_now());
    assert_eq!(decided.how(), Some("window end"));
    assert_eq!(at(REVIEWED, &[]).how(), None);
}

#[test]
fn with_both_labels_hold_wins() {
    assert_eq!(at(REVIEWED, &["go", "hold"]), Window::Held);
}

#[test]
fn labels_match_case_insensitively() {
    assert_eq!(at(REVIEWED, &["Hold"]), Window::Held);
    assert_eq!(at(REVIEWED, &["GO"]), Window::Go);
}

#[test]
fn go_on_a_pr_failing_another_rule_is_not_armed_and_gives_that_rules_reason() {
    let start = window::start(&[review("success", REVIEWED)]);
    let reasons = ["its issue #7 is size L".to_owned()];

    let decided = window::decide(&labels(&["go"]), start, &reasons, false, REVIEWED);

    assert_eq!(decided, Window::Refused(reasons.to_vec()));
    assert!(!decided.arm_now());
}

#[test]
fn go_never_skips_a_missing_clean_review() {
    let start = window::start(&[review("failure", REVIEWED)]);

    let decided = window::decide(&labels(&["go"]), start, &[], false, REVIEWED);

    assert!(matches!(decided, Window::NotEligible(_)), "{decided:?}");
}

#[test]
fn hold_on_an_armed_pr_is_held_whatever_else_it_fails_so_the_caller_disarms() {
    // An armed PR: past its window, or `go`; the user labels it `hold` afterwards.
    assert_eq!(at(REVIEWED + 3 * HOUR, &["go", "HOLD"]), Window::Held);
    let decided = window::decide(
        &labels(&["hold"]),
        window::start(&[]),
        &["it changes Cargo.lock".to_owned()],
        false,
        REVIEWED,
    );
    assert_eq!(decided, Window::Held);
}

#[test]
fn a_go_the_loop_could_not_remove_counts_as_not_go() {
    let start = window::start(&[review("success", REVIEWED)]);

    let decided = window::decide(&labels(&["go"]), start, &[], true, REVIEWED);

    assert_eq!(
        decided,
        Window::Waiting {
            until: REVIEWED + 2 * HOUR
        }
    );
}

// --- Read from GitHub ---

#[test]
fn the_window_of_a_pr_is_read_from_its_labels_and_its_heads_statuses() {
    let gh = FakeGitHub::default()
        .with_pr_labels(12, &["Go"])
        .with_statuses(HEAD, vec![review("success", REVIEWED)]);

    assert_eq!(
        window::read(&gh, REPO, 12, HEAD, &[], false, REVIEWED),
        Window::Go
    );
}

#[test]
fn labels_that_cant_be_read_make_it_not_eligible() {
    let gh = FakeGitHub::default()
        .failing_pr_labels("HTTP 502")
        .with_statuses(HEAD, vec![review("success", REVIEWED)]);

    let decided = window::read(&gh, REPO, 12, HEAD, &[], false, REVIEWED + 3 * HOUR);

    assert!(
        matches!(&decided, Window::NotEligible(why) if why.contains("labels") && why.contains("HTTP 502")),
        "{decided:?}"
    );
}

#[test]
fn statuses_that_cant_be_read_make_it_not_eligible_unless_held() {
    let gh = FakeGitHub::default()
        .with_pr_labels(12, &["go"])
        .failing_commit_statuses("HTTP 502");

    let decided = window::read(&gh, REPO, 12, HEAD, &[], false, REVIEWED);

    assert!(
        matches!(&decided, Window::NotEligible(why) if why.contains("HTTP 502")),
        "{decided:?}"
    );
    let held = FakeGitHub::default()
        .with_pr_labels(12, &["hold"])
        .failing_commit_statuses("HTTP 502");
    assert_eq!(
        window::read(&held, REPO, 12, HEAD, &[], false, REVIEWED),
        Window::Held
    );
}
