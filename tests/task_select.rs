//! M2b slice 3: issue selection (spec §8). Golden cases ported from
//! the removed `SelectIssues` Pester tests, plus both-direction `Related`, the explicit
//! list and the `--workers` cap.

use std::collections::HashMap;

use sbxm::github::{Issue, PrState};
use sbxm::task::select::{Reason, Selection, select};

fn issue(number: u32, labels: &[&str], body: &str) -> Issue {
    Issue {
        number,
        open: true,
        title: format!("issue {number}"),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        body: body.to_owned(),
    }
}

fn plain(number: u32) -> Issue {
    issue(number, &["should-fix"], "")
}

fn pick(
    open: &[Issue],
    in_progress: &[u32],
    explicit: Option<&[u32]>,
    workers: usize,
) -> Selection {
    select(open, in_progress, explicit, workers, &HashMap::new())
}

fn pick_with_prs(
    open: &[Issue],
    explicit: Option<&[u32]>,
    workers: usize,
    prs: &[(u32, PrState)],
) -> Selection {
    select(open, &[], explicit, workers, &prs.iter().copied().collect())
}

#[test]
fn must_fix_comes_before_should_fix_up_to_the_worker_count() {
    let open = [plain(1), issue(2, &["must-fix"], ""), plain(3)];
    assert_eq!(pick(&open, &[], None, 2).picks, [2, 1]);
}

#[test]
fn an_unlabelled_issue_ranks_after_the_labelled_ones() {
    let open = [issue(1, &[], ""), plain(2)];
    assert_eq!(pick(&open, &[], None, 2).picks, [2, 1]);
}

#[test]
fn the_best_label_decides_the_rank() {
    let open = [plain(1), issue(2, &["bug", "must-fix"], "")];
    assert_eq!(pick(&open, &[], None, 2).picks, [2, 1]);
}

#[test]
fn a_question_is_skipped_with_its_reason() {
    let open = [issue(1, &["question", "must-fix"], ""), plain(2)];
    let selection = pick(&open, &[], None, 2);
    assert_eq!(selection.picks, [2]);
    assert_eq!(selection.skips, [(1, Reason::Question)]);
    assert_eq!(
        Reason::Question.to_string(),
        "a question, needs your answer first"
    );
}

#[test]
fn an_open_dependency_blocks_an_issue() {
    let open = [plain(1), issue(2, &["should-fix"], "**Depends on:** #1")];
    let selection = pick(&open, &[], None, 2);
    assert_eq!(selection.picks, [1]);
    assert_eq!(selection.skips, [(2, Reason::Blocked(vec![1]))]);
    assert_eq!(
        Reason::Blocked(vec![1, 4]).to_string(),
        "blocked by open #1, #4"
    );
}

#[test]
fn a_closed_dependency_does_not_block() {
    let open = [issue(2, &["should-fix"], "**Depends on:** #1")];
    assert_eq!(pick(&open, &[], None, 2).picks, [2]);
}

#[test]
fn every_issue_number_on_the_field_line_counts_and_only_that_line() {
    let body = "intro mentioning #9\n**Depends on:** #5, #3 and #5\nlater #7";
    let open = [plain(3), plain(5), plain(7), plain(9), issue(8, &[], body)];
    let selection = pick(&open, &[], None, 9);
    assert!(selection.skips.contains(&(8, Reason::Blocked(vec![3, 5]))));
}

#[test]
fn a_field_not_at_the_start_of_a_line_is_ignored() {
    let open = [plain(1), issue(2, &[], "see **Depends on:** #1")];
    assert_eq!(pick(&open, &[], None, 9).picks, [1, 2]);
}

#[test]
fn an_issue_with_a_task_is_skipped() {
    let open = [plain(1), plain(2)];
    let selection = pick(&open, &[1], None, 2);
    assert_eq!(selection.picks, [2]);
    assert_eq!(selection.skips, [(1, Reason::HasTask)]);
    assert_eq!(Reason::HasTask.to_string(), "already has a task");
}

#[test]
fn related_written_on_the_tasks_issue_blocks_the_other() {
    let open = [issue(1, &["should-fix"], "**Related:** #2"), plain(2)];
    let selection = pick(&open, &[1], None, 2);
    assert!(selection.picks.is_empty());
    assert_eq!(selection.skips[1], (2, Reason::RelatedToTask(vec![1])));
    assert_eq!(
        Reason::RelatedToTask(vec![1]).to_string(),
        "related to #1, which has a task"
    );
}

#[test]
fn related_written_on_its_own_issue_blocks_it_too() {
    let open = [plain(1), issue(2, &["should-fix"], "**Related:** #1")];
    let selection = pick(&open, &[1], None, 2);
    assert!(selection.picks.is_empty());
    assert!(
        selection
            .skips
            .contains(&(2, Reason::RelatedToTask(vec![1])))
    );
}

#[test]
fn two_related_issues_are_not_started_in_the_same_run() {
    let open = [issue(1, &["should-fix"], "**Related:** #2"), plain(2)];
    let selection = pick(&open, &[], None, 2);
    assert_eq!(selection.picks, [1]);
    assert_eq!(selection.skips, [(2, Reason::RelatedToTask(vec![1]))]);
}

#[test]
fn the_worker_cap_stops_the_walk_without_skip_reasons_for_the_rest() {
    let open = [plain(1), plain(2), issue(3, &["question"], "")];
    let selection = pick(&open, &[], None, 2);
    assert_eq!(selection.picks, [1, 2]);
    assert!(selection.skips.is_empty());
}

#[test]
fn an_explicit_list_picks_only_those_and_ignores_the_cap() {
    let open = [plain(1), plain(2), plain(3), plain(4)];
    let selection = pick(&open, &[], Some(&[4, 2, 3]), 1);
    assert_eq!(selection.picks, [2, 3, 4]);
}

#[test]
fn an_explicit_number_that_is_not_open_is_reported() {
    let open = [plain(1)];
    let selection = pick(&open, &[], Some(&[1, 9]), 1);
    assert_eq!(selection.picks, [1]);
    assert_eq!(selection.not_open, [9]);
}

#[test]
fn explicit_issues_still_obey_the_other_rules() {
    let open = [issue(1, &["question"], ""), plain(2)];
    let selection = pick(&open, &[2], Some(&[1, 2]), 1);
    assert!(selection.picks.is_empty());
    assert_eq!(
        selection.skips,
        [(2, Reason::HasTask), (1, Reason::Question)]
    );
}

#[test]
fn nothing_open_selects_nothing() {
    let selection = pick(&[], &[], None, 3);
    assert!(
        selection.picks.is_empty() && selection.skips.is_empty() && selection.not_open.is_empty()
    );
}

// Decision 169 (c): must-fix issues of an open PR come first.

#[test]
fn a_must_fix_issue_of_an_open_pr_comes_before_every_other_issue() {
    let open = [
        issue(1, &["must-fix"], ""),
        plain(2),
        issue(5, &["must-fix"], "PR: #40\nbody"),
    ];
    let selection = pick_with_prs(&open, None, 3, &[(40, PrState::Open)]);
    assert_eq!(selection.picks, [5, 1, 2]);
}

#[test]
fn ties_among_open_pr_must_fix_issues_keep_the_number_order() {
    let open = [
        issue(7, &["must-fix"], "PR: #41"),
        issue(3, &["must-fix"], "PR: #40"),
        issue(1, &["must-fix"], ""),
    ];
    let prs = [(40, PrState::Open), (41, PrState::Open)];
    assert_eq!(pick_with_prs(&open, None, 3, &prs).picks, [3, 7, 1]);
}

#[test]
fn the_worker_cap_takes_the_open_pr_must_fix_issues_first() {
    let open = [
        issue(1, &["must-fix"], ""),
        issue(9, &["must-fix"], "PR: #40"),
    ];
    let selection = pick_with_prs(&open, None, 1, &[(40, PrState::Open)]);
    assert_eq!(selection.picks, [9]);
}

#[test]
fn a_should_fix_issue_of_an_open_pr_keeps_its_usual_rank() {
    let open = [
        issue(1, &["must-fix"], ""),
        issue(2, &["should-fix"], "PR: #40"),
    ];
    let selection = pick_with_prs(&open, None, 2, &[(40, PrState::Open)]);
    assert_eq!(selection.picks, [1, 2]);
}

#[test]
fn a_pr_line_that_is_not_the_first_line_does_not_count() {
    let open = [
        issue(1, &["must-fix"], ""),
        issue(2, &["must-fix"], "intro\nPR: #40"),
    ];
    let selection = pick_with_prs(&open, None, 2, &[(40, PrState::Open)]);
    assert_eq!(selection.picks, [1, 2]);
}

#[test]
fn a_must_fix_issue_of_a_merged_or_closed_pr_is_skipped_with_its_reason() {
    let open = [
        issue(1, &["must-fix"], "PR: #40"),
        issue(2, &["must-fix"], "PR: #41"),
        plain(3),
    ];
    let prs = [(40, PrState::Merged), (41, PrState::Closed)];
    let selection = pick_with_prs(&open, None, 3, &prs);
    assert_eq!(selection.picks, [3]);
    assert_eq!(
        selection.skips,
        [
            (1, Reason::PrNotOpen(40, PrState::Merged)),
            (2, Reason::PrNotOpen(41, PrState::Closed)),
        ]
    );
    assert_eq!(
        Reason::PrNotOpen(40, PrState::Merged).to_string(),
        "its PR #40 is merged"
    );
    assert_eq!(
        Reason::PrNotOpen(41, PrState::Closed).to_string(),
        "its PR #41 is closed"
    );
}

#[test]
fn the_other_rules_still_apply_to_an_open_pr_must_fix_issue() {
    let open = [
        plain(1),
        issue(2, &["must-fix", "question"], "PR: #40"),
        issue(3, &["must-fix"], "PR: #40\n**Depends on:** #1"),
        issue(4, &["must-fix"], "PR: #40"),
    ];
    let selection = select(
        &open,
        &[4],
        None,
        9,
        &[(40, PrState::Open)].into_iter().collect(),
    );
    assert_eq!(selection.picks, [1]);
    assert_eq!(
        selection.skips,
        [
            (2, Reason::Question),
            (3, Reason::Blocked(vec![1])),
            (4, Reason::HasTask),
        ]
    );
}

#[test]
fn an_explicit_list_still_names_any_issue_whatever_its_pr() {
    let open = [
        issue(1, &["must-fix"], "PR: #40"),
        issue(2, &["must-fix"], "PR: #41"),
        plain(3),
    ];
    let prs = [(40, PrState::Merged), (41, PrState::Open)];
    let selection = pick_with_prs(&open, Some(&[3, 1]), 1, &prs);
    assert_eq!(selection.picks, [1, 3]);
    assert!(selection.skips.is_empty());
}

#[test]
fn a_must_fix_issue_whose_pr_state_is_unknown_is_skipped_but_an_explicit_one_is_not() {
    let open = [issue(1, &["must-fix"], "PR: #40"), plain(2)];

    let automatic = pick_with_prs(&open, None, 2, &[]);
    assert_eq!(automatic.picks, [2]);
    assert_eq!(automatic.skips, [(1, Reason::PrUnreadable(40))]);
    assert_eq!(
        Reason::PrUnreadable(40).to_string(),
        "its PR #40 couldn't be read"
    );

    let explicit = pick_with_prs(&open, Some(&[1]), 1, &[]);
    assert_eq!(explicit.picks, [1]);
    assert!(explicit.skips.is_empty());
}
