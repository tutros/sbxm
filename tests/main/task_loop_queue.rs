//! Issue 320: the pure parts of `sbxm task loop` — the queue's order (decision 2, 245(b)), the
//! numbers a `Depends on:` line names, the usage-limit match (decision 6), the log rows (245(h))
//! and the empty-queue report (245(c)).

use crate::common;

use std::fs;

use common::task_fixture::open_issue;
use sbxm::task::queue::{
    self, Report, Row, Skipped, append_row, is_usage_limit, pr_number, queue_order, span,
};

#[test]
fn the_queue_is_the_labeled_issues_by_size_then_oldest() {
    let open = vec![
        open_issue(9, &["ready-for-worker", "size:M"], ""),
        open_issue(4, &["ready-for-worker", "size:L"], ""),
        open_issue(7, &["ready-for-worker", "size:S"], ""),
        open_issue(2, &["ready-for-worker", "size:M"], ""),
        open_issue(3, &["size:S"], ""),
        open_issue(8, &["ready-for-worker"], ""),
        open_issue(1, &["ready-for-worker", "size:S"], ""),
    ];
    // Unsized after L; #3 has no label and isn't in the queue.
    assert_eq!(queue_order(&open), [1, 7, 2, 9, 4, 8]);
}

#[test]
fn an_empty_queue_is_empty() {
    let open = vec![open_issue(3, &["size:S"], "")];
    assert!(queue_order(&open).is_empty());
}

#[test]
fn depends_on_reads_every_number_of_its_line() {
    let body = "Why\n\n**Depends on:** #341 (worker sandbox removal), and PR #377 (commits).\n\
                **Related:** #12\n";
    assert_eq!(queue::depends_on(body), [341, 377]);
    assert!(queue::depends_on("no such line").is_empty());
}

#[test]
fn a_usage_limit_is_told_by_its_words_in_any_case() {
    assert!(is_usage_limit(
        "worker failed: Claude AI usage limit reached|1761000000"
    ));
    assert!(is_usage_limit(
        "You've hit your Usage Limit. Try again later."
    ));
    assert!(!is_usage_limit("worker failed: exit 1"));
    assert!(!is_usage_limit("the disk usage is high; limit it"));
}

#[test]
fn a_pr_number_is_read_from_its_url() {
    assert_eq!(pr_number("https://github.com/o/r/pull/293"), Some(293));
    assert_eq!(pr_number("https://github.com/o/r/pull/293/"), Some(293));
    assert_eq!(pr_number("https://github.com/o/r/issues/293"), None);
    assert_eq!(pr_number("not a url"), None);
}

#[test]
fn a_span_has_minutes_and_seconds_and_hours_when_needed() {
    assert_eq!(span(0), "0s");
    assert_eq!(span(59), "59s");
    assert_eq!(span(555), "9m15s");
    assert_eq!(span(3_600), "1h00m00s");
    assert_eq!(span(3_725), "1h02m05s");
}

fn row(issue: u32) -> Row {
    Row {
        issue,
        title: "Fix | it".into(),
        size: Some("S".into()),
        slot: None,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: Some(1_761_232_113),
        worker_s: Some(261),
        reviewer_s: Some(197),
        rounds: Some(1),
        must_fix: Some(0),
        outcome: "PR #293, clean".into(),
        stops: vec![("disk".into(), 120)],
        idle_before: Some(190),
    }
}

#[test]
fn a_new_log_gets_its_header_then_the_row() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("sdlc/batches/2025-10-23-loop.md");

    assert!(append_row(&path, "o/r", &row(291)).unwrap());

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Loop log"), "{text}");
    assert!(
        text.contains(
            "| # | Issue | Size | Slot | Started | Ready (time to ready) | Worker / reviewer s | Rounds \
             (must-fix) | Outcome | Stops (cause, duration) | Idle before | User interventions |"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "| 1 | #291 Fix \\| it | S | - | 14:59:18 | 15:08:33 (9m15s) | 261 / 197 | 1 (0) | PR #293, \
             clean | disk, 2m00s | 3m10s | unknown |"
        ),
        "{text}"
    );
}

#[test]
fn rows_are_numbered_and_an_issue_already_logged_is_not_logged_again() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");
    assert!(append_row(&path, "o/r", &row(291)).unwrap());
    assert!(append_row(&path, "o/r", &row(292)).unwrap());
    assert!(
        !append_row(&path, "o/r", &row(291)).unwrap(),
        "already logged"
    );

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("#291 ").count(), 1, "{text}");
    assert!(text.contains("| 2 | #292 "), "{text}");
}

#[test]
fn a_task_that_never_got_ready_shows_dashes_and_no_stops_shows_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");
    let parked = Row {
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds: None,
        must_fix: None,
        size: None,
        outcome: "parked at working".into(),
        stops: Vec::new(),
        idle_before: None,
        ..row(5)
    };
    append_row(&path, "o/r", &parked).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(
            "| 1 | #5 Fix \\| it | unknown | - | 14:59:18 | - | - | - | parked at working | none | - | \
             unknown |"
        ),
        "{text}"
    );
}

#[test]
fn the_report_lists_the_skipped_and_the_almost_ready_with_their_questions() {
    let open = vec![
        open_issue(
            10,
            &["size:M"],
            "Why\n\n## Open points\n\n- Should it warn or refuse?\n- Done: the format.\n",
        ),
        open_issue(11, &["question", "size:S"], "Which one?"),
        open_issue(12, &["ready-for-worker", "size:S"], ""),
        open_issue(13, &[], "Not sized: is this wanted?"),
    ];
    let skipped = vec![Skipped {
        issue: 12,
        reason: "depends on #341, not merged".into(),
    }];
    let text = queue::report(&Report {
        open: &open,
        skipped: &skipped,
    });

    assert!(
        text.contains("#12 Fix 12: depends on #341, not merged"),
        "{text}"
    );
    assert!(text.contains("#10 Fix 10 (size:M)"), "{text}");
    assert!(text.contains("Should it warn or refuse?"), "{text}");
    assert!(!text.contains("Done: the format"), "{text}");
    assert!(text.contains("#11 Fix 11 (size:S)"), "{text}");
    assert!(text.contains("labeled question"), "{text}");
    assert!(
        !text.contains("#13"),
        "an unsized issue isn't almost ready: {text}"
    );
}

#[test]
fn the_report_of_nothing_says_so() {
    let text = queue::report(&Report {
        open: &[],
        skipped: &[],
    });
    assert!(text.contains("no almost-ready issues"), "{text}");
}

#[test]
fn a_usage_limit_counts_in_the_error_or_a_failure_line_only() {
    assert!(queue::failure_says_usage_limit("usage limit reached", ""));
    assert!(queue::failure_says_usage_limit(
        "1 task failed",
        "issue-3: worker claude failed: claude reported an error: Claude AI usage limit reached\n"
    ));
    // The issue's own words, printed on the way, are not a failure.
    assert!(!queue::failure_says_usage_limit(
        "1 task failed",
        "task 1: #320 Wait out the usage limit\nissue-3: worker claude failed: exit 1\n"
    ));
}
