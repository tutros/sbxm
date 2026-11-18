//! Issue 725: the readiness rules (`src/task/ready.rs`) one by one with their
//! boundary cases, then `sbxm task ready-check --issue N [--json]`.

use crate::common;

use common::task_fixture::open_issue;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, LabelEvent, MergedPr, PrFile, PrFiles};
use sbxm::task::ready::{self, Status, Verdict};

/// A body every rule passes, for a test that changes one part of it.
const READY_BODY: &str = "**Size:** M: a reason.\n\n\
**Depends on:** none\n\n\
## Acceptance criteria\n\n\
- [ ] It works\n";

fn ready_issue(body: &str) -> Issue {
    open_issue(7, &["ready-for-worker", "size:M"], body)
}

fn verdict(issue: &Issue, rule: &str) -> Verdict {
    ready::body_rules(issue)
        .into_iter()
        .find(|v| v.rule == rule)
        .unwrap_or_else(|| panic!("no verdict for rule {rule}"))
}

fn status(issue: &Issue, rule: &str) -> Status {
    verdict(issue, rule).status
}

#[test]
fn a_ready_body_passes_every_rule() {
    let issue = ready_issue(READY_BODY);
    let verdicts = ready::body_rules(&issue);
    let rules: Vec<&str> = verdicts.iter().map(|v| v.rule).collect();
    assert_eq!(
        rules,
        [
            "size",
            "depends-on",
            "real-check",
            "open-points",
            "acceptance",
            "files"
        ]
    );
    for v in &verdicts {
        assert!(
            matches!(v.status, Status::Pass | Status::Skip),
            "{} gave {:?}: {}",
            v.rule,
            v.status,
            v.reason
        );
    }
    assert!(ready::failures(&issue).is_empty());
}

// Rule 1: one size.

#[test]
fn size_one_label_and_a_matching_line_passes() {
    assert_eq!(status(&ready_issue(READY_BODY), "size"), Status::Pass);
}

#[test]
fn size_a_label_and_a_different_line_fails() {
    let body = READY_BODY.replace("**Size:** M", "**Size:** L");
    let v = verdict(&ready_issue(&body), "size");
    assert_eq!(v.status, Status::Fail);
    assert!(
        v.reason.contains("size:M") && v.reason.contains('L'),
        "{}",
        v.reason
    );
}

#[test]
fn size_a_label_and_no_line_fails() {
    let body = READY_BODY.replace("**Size:** M: a reason.\n", "");
    let v = verdict(&ready_issue(&body), "size");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("**Size:**"), "{}", v.reason);
}

#[test]
fn size_no_label_fails() {
    let issue = open_issue(7, &["ready-for-worker"], READY_BODY);
    let v = verdict(&issue, "size");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("no size label"), "{}", v.reason);
}

#[test]
fn size_two_labels_fail() {
    let issue = open_issue(7, &["size:S", "size:M"], READY_BODY);
    let v = verdict(&issue, "size");
    assert_eq!(v.status, Status::Fail);
    assert!(
        v.reason.contains("more than one size label"),
        "{}",
        v.reason
    );
}

// Rule 2: `**Depends on:**`.

fn depends(value: Option<&str>) -> Verdict {
    let line = value.map_or_else(String::new, |v| format!("**Depends on:**{v}\n"));
    let body = READY_BODY.replace("**Depends on:** none\n", &line);
    verdict(&ready_issue(&body), "depends-on")
}

#[test]
fn depends_on_missing_fails() {
    let v = depends(None);
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("no **Depends on:** line"), "{}", v.reason);
}

#[test]
fn depends_on_none_passes() {
    assert_eq!(depends(Some(" none")).status, Status::Pass);
    assert_eq!(depends(Some(" None.")).status, Status::Pass);
}

#[test]
fn depends_on_a_list_of_numbers_passes() {
    assert_eq!(depends(Some(" #1, #2")).status, Status::Pass);
}

#[test]
fn depends_on_a_number_with_text_passes() {
    assert_eq!(depends(Some(" #1 (why)")).status, Status::Pass);
    assert_eq!(
        depends(Some(" #709, #715 (both change it)")).status,
        Status::Pass
    );
}

#[test]
fn depends_on_malformed_fails() {
    for value in [" PR #377", " soon", "", " #", " #1, soon", " nonetheless"] {
        let v = depends(Some(value));
        assert_eq!(v.status, Status::Fail, "{value:?}");
        assert!(v.reason.contains("Depends on"), "{}", v.reason);
    }
}

// Rule 3: `**Real check:**`.

#[test]
fn real_check_absent_is_a_skip() {
    assert_eq!(status(&ready_issue(READY_BODY), "real-check"), Status::Skip);
}

#[test]
fn real_check_with_a_value_passes() {
    let body = format!("{READY_BODY}\n**Real check:** run `just real-test`\n");
    assert_eq!(status(&ready_issue(&body), "real-check"), Status::Pass);
}

#[test]
fn real_check_with_the_check_on_the_next_line_passes() {
    let body = format!("{READY_BODY}\n**Real check:**\n- run it by hand\n");
    assert_eq!(status(&ready_issue(&body), "real-check"), Status::Pass);
}

#[test]
fn real_check_empty_fails() {
    let body = format!("{READY_BODY}\n**Real check:**   \n");
    let v = verdict(&ready_issue(&body), "real-check");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("empty"), "{}", v.reason);
}

// Rule 4: nothing left open.

#[test]
fn open_points_absent_passes() {
    assert_eq!(
        status(&ready_issue(READY_BODY), "open-points"),
        Status::Pass
    );
}

#[test]
fn open_points_to_decide_in_a_heading_fails() {
    let body = format!("{READY_BODY}\n## To decide when building\n\nsomething\n");
    let v = verdict(&ready_issue(&body), "open-points");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("to decide"), "{}", v.reason);
}

#[test]
fn open_points_to_decide_in_lower_case_fails() {
    let body = format!("{READY_BODY}\nThe name is to decide.\n");
    assert_eq!(status(&ready_issue(&body), "open-points"), Status::Fail);
}

#[test]
fn open_points_an_open_line_fails() {
    let body = format!("{READY_BODY}\n**Open:** which flag name\n");
    let v = verdict(&ready_issue(&body), "open-points");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("**Open:**"), "{}", v.reason);
}

#[test]
fn open_points_an_open_points_heading_fails() {
    let body = format!("{READY_BODY}\n## Open points\n\n1. a\n");
    assert_eq!(status(&ready_issue(&body), "open-points"), Status::Fail);
}

// Rule 5: acceptance criteria.

#[test]
fn acceptance_a_heading_with_boxes_passes() {
    assert_eq!(status(&ready_issue(READY_BODY), "acceptance"), Status::Pass);
}

#[test]
fn acceptance_a_heading_with_no_boxes_fails() {
    let body = READY_BODY.replace("- [ ] It works\n", "It works.\n");
    let v = verdict(&ready_issue(&body), "acceptance");
    assert_eq!(v.status, Status::Fail);
    assert!(v.reason.contains("no `- [ ]` box"), "{}", v.reason);
}

#[test]
fn acceptance_boxes_under_a_later_heading_dont_count() {
    let body = READY_BODY.replace(
        "- [ ] It works\n",
        "It works.\n\n## Boundary cases\n\n- [ ] one\n",
    );
    assert_eq!(status(&ready_issue(&body), "acceptance"), Status::Fail);
}

#[test]
fn acceptance_no_heading_fails() {
    let body = READY_BODY.replace("## Acceptance criteria\n", "");
    let v = verdict(&ready_issue(&body), "acceptance");
    assert_eq!(v.status, Status::Fail);
    assert!(
        v.reason.contains("no Acceptance criteria heading"),
        "{}",
        v.reason
    );
}

// Rule 7: `**Files:**`, advice only.

#[test]
fn files_absent_is_a_skip() {
    assert_eq!(status(&ready_issue(READY_BODY), "files"), Status::Skip);
}

#[test]
fn files_a_list_of_backticked_paths_passes() {
    let body = format!("{READY_BODY}\n**Files:** `src/cli.rs`, `src/task/ready.rs` (new)\n");
    assert_eq!(status(&ready_issue(&body), "files"), Status::Pass);
}

#[test]
fn files_a_malformed_line_warns() {
    for line in [
        "src/cli.rs, src/main.rs",
        "`src/cli.rs` and `src/main.rs`",
        "",
    ] {
        let body = format!("{READY_BODY}\n**Files:** {line}\n");
        let issue = ready_issue(&body);
        let v = verdict(&issue, "files");
        assert_eq!(v.status, Status::Warn, "{line:?}");
        // A warn never makes the issue unready.
        assert!(ready::failures(&issue).is_empty());
    }
}

#[test]
fn failures_name_every_failed_rule() {
    let issue = open_issue(7, &[], "nothing here");
    let failures = ready::failures(&issue);
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(failures[0].contains("no size label"), "{failures:?}");
}

#[test]
fn a_fenced_example_is_not_read() {
    let body =
        format!("{READY_BODY}\n```\n**Open:** an example\n**Real check:**\nto decide\n```\n");
    let issue = ready_issue(&body);
    assert_eq!(status(&issue, "open-points"), Status::Pass);
    assert_eq!(status(&issue, "real-check"), Status::Skip);
}

// Rule 6: paths on the default branch, one listing read.

fn with_paths(body: &str) -> Vec<Verdict> {
    let gh = FakeGitHub::default().with_repo_files(&[
        ("src/cli.rs", ""),
        ("src/task/queue.rs", ""),
        ("README.md", ""),
    ]);
    ready::check("o/r", &gh, &ready_issue(&format!("{READY_BODY}\n{body}\n")))
}

fn paths_verdict(body: &str) -> Verdict {
    with_paths(body)
        .into_iter()
        .find(|v| v.rule == "paths")
        .unwrap()
}

#[test]
fn check_runs_every_rule_in_order() {
    let rules: Vec<&str> = with_paths("").iter().map(|v| v.rule).collect();
    assert_eq!(
        rules,
        [
            "size",
            "depends-on",
            "real-check",
            "open-points",
            "acceptance",
            "paths",
            "files",
            "staleness"
        ]
    );
}

#[test]
fn paths_one_that_exists_passes() {
    let v = paths_verdict("Change `src/cli.rs` and `README.md`.");
    assert_eq!(v.status, Status::Pass, "{}", v.reason);
}

#[test]
fn paths_a_new_one_is_not_looked_for() {
    let v = paths_verdict("Add `src/task/ready.rs` (new) next to `src/task/queue.rs`.");
    assert_eq!(v.status, Status::Pass, "{}", v.reason);
    let v = paths_verdict("Add `src/task/ready.rs` (new).");
    assert_eq!(v.status, Status::Skip, "{}", v.reason);
}

#[test]
fn paths_a_missing_one_warns() {
    let v = paths_verdict("Change `src/gone.rs` and `src/cli.rs`.");
    assert_eq!(v.status, Status::Warn);
    assert!(v.reason.contains("src/gone.rs"), "{}", v.reason);
    assert!(!v.reason.contains("src/cli.rs"), "{}", v.reason);
}

#[test]
fn paths_a_folder_ending_in_a_slash_exists_through_its_files() {
    let v = paths_verdict("In `src/task/` and `src/`.");
    assert_eq!(v.status, Status::Pass, "{}", v.reason);
    let v = paths_verdict("In `src/nothing/`.");
    assert_eq!(v.status, Status::Warn, "{}", v.reason);
}

#[test]
fn paths_words_in_backticks_that_are_no_paths_are_left_alone() {
    let v = paths_verdict(
        "Run `cargo test`, pass `--json`, see `size:*`, `- [ ]` and `https://x.y/z`.",
    );
    assert_eq!(v.status, Status::Skip, "{}", v.reason);
}

#[test]
fn paths_one_listing_is_read_from_the_default_branch() {
    let gh = FakeGitHub::default()
        .with_default_branch("trunk")
        .with_repo_files(&[("src/cli.rs", "")]);
    ready::check(
        "o/r",
        &gh,
        &ready_issue(&format!("{READY_BODY}\n`src/cli.rs`, `src/main.rs`\n")),
    );
    let listings: Vec<GhCall> = gh
        .calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::RepoPaths(..)))
        .collect();
    assert_eq!(listings, [GhCall::RepoPaths("o/r".into(), "trunk".into())]);
}

#[test]
fn paths_a_listing_that_fails_warns() {
    let gh = FakeGitHub::default();
    let issue = ready_issue(&format!("{READY_BODY}\n`src/cli.rs`\n"));
    let v = ready::check("o/r", &gh, &issue)
        .into_iter()
        .find(|v| v.rule == "paths")
        .unwrap();
    assert_eq!(v.status, Status::Warn);
    assert!(v.reason.contains("not scripted"), "{}", v.reason);
}

// Rule 8: staleness (decision 295), the PRs merged since the label that changed a named path.

/// 2025-11-10T12:00:00Z, when the issue was last labeled.
const LABELED_AT: u64 = 1_762_776_000;

fn pr_files(names: &[&str]) -> PrFiles {
    PrFiles {
        changed_files: u32::try_from(names.len()).unwrap(),
        files: names
            .iter()
            .map(|n| PrFile {
                filename: (*n).to_owned(),
                previous_filename: None,
            })
            .collect(),
    }
}

/// A repo where #7 was labeled at [`LABELED_AT`] and PR #20 merged an hour before it (touching
/// `src/cli.rs`), #21 an hour after it (touching `src/cli.rs` and `docs/x.md`) and #22 after it
/// (touching only `Cargo.toml`).
fn stale_repo() -> FakeGitHub {
    let gh = FakeGitHub::default()
        .with_repo_files(&[("src/cli.rs", ""), ("src/task/queue.rs", "")])
        .with_merged_prs(vec![
            MergedPr {
                number: 20,
                merged_at: LABELED_AT - 3_600,
            },
            MergedPr {
                number: 21,
                merged_at: LABELED_AT + 3_600,
            },
            MergedPr {
                number: 22,
                merged_at: LABELED_AT + 7_200,
            },
        ])
        .with_pr_files(20, pr_files(&["src/cli.rs"]))
        .with_pr_files(21, pr_files(&["src/cli.rs", "docs/x.md"]))
        .with_pr_files(22, pr_files(&["Cargo.toml"]));
    gh.set_label_events(
        7,
        vec![
            LabelEvent {
                label: "ready-for-worker".into(),
                added: true,
                at: LABELED_AT - 86_400,
            },
            LabelEvent {
                label: "ready-for-worker".into(),
                added: false,
                at: LABELED_AT - 600,
            },
            LabelEvent {
                label: "ready-for-worker".into(),
                added: true,
                at: LABELED_AT,
            },
        ],
    );
    gh
}

fn staleness(gh: &FakeGitHub, issue: &Issue) -> Verdict {
    ready::check("o/r", gh, issue)
        .into_iter()
        .find(|v| v.rule == "staleness")
        .unwrap()
}

#[test]
fn staleness_a_pr_merged_since_the_label_that_changed_a_named_path_warns() {
    let gh = stale_repo();
    let issue = ready_issue(&format!(
        "{READY_BODY}\nChange `src/cli.rs` and `src/task/`.\n"
    ));

    let v = staleness(&gh, &issue);

    assert_eq!(v.status, Status::Warn, "{}", v.reason);
    assert!(v.reason.contains("#21 (src/cli.rs)"), "{}", v.reason);
    // Merged before the label was last added, or touching no named path.
    assert!(
        !v.reason.contains("#20") && !v.reason.contains("#22"),
        "{}",
        v.reason
    );
    assert!(ready::failures(&issue).is_empty());
    // The merged PRs are listed from the day the label was last added.
    assert!(
        gh.calls()
            .contains(&GhCall::PrsMergedSince("o/r".into(), "2025-11-10".into())),
        "{:?}",
        gh.calls()
    );
}

#[test]
fn staleness_a_folder_matches_the_files_in_it() {
    let gh = stale_repo();
    let issue = ready_issue(&format!("{READY_BODY}\nIn `docs/`.\n"));
    let v = staleness(&gh, &issue);
    assert_eq!(v.status, Status::Warn, "{}", v.reason);
    assert!(v.reason.contains("#21 (docs/x.md)"), "{}", v.reason);
}

#[test]
fn staleness_no_pr_changed_a_named_path_passes() {
    let gh = stale_repo();
    let issue = ready_issue(&format!("{READY_BODY}\nChange `src/task/queue.rs`.\n"));
    assert_eq!(staleness(&gh, &issue).status, Status::Pass);
}

#[test]
fn staleness_without_the_label_or_a_named_path_is_a_skip() {
    let gh = stale_repo();
    let unlabeled = open_issue(7, &["size:M"], &format!("{READY_BODY}\n`src/cli.rs`\n"));
    assert_eq!(staleness(&gh, &unlabeled).status, Status::Skip);
    assert_eq!(
        staleness(&gh, &ready_issue(READY_BODY)).status,
        Status::Skip
    );
}

#[test]
fn staleness_a_read_that_fails_warns() {
    let gh = stale_repo();
    gh.set_label_events_failure(Some("events are down"));
    let issue = ready_issue(&format!("{READY_BODY}\n`src/cli.rs`\n"));
    let v = staleness(&gh, &issue);
    assert_eq!(v.status, Status::Warn);
    assert!(v.reason.contains("events are down"), "{}", v.reason);
}
