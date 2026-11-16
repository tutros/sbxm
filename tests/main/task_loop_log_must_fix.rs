//! Issue 697: the loop log's "Rounds (must-fix)" cell lists every round's must-fix count, in
//! order, read from the task's `review-<n>.md` files as `sbxm task facts` reads them; `?` for a
//! round whose count can't be read, `-` with no review round.

use std::fs;

use sbxm::task::queue::{Row, append_row};
use sbxm::task::review;

fn row(rounds: Option<u32>, must_fix: Vec<Option<u32>>) -> Row {
    Row {
        issue: 7,
        title: "Fix it".into(),
        size: Some("S".into()),
        slot: None,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds,
        must_fix,
        outcome: "PR #9, clean".into(),
        stops: Vec::new(),
        idle_before: None,
    }
}

/// The Rounds (must-fix) cell of `row`'s log row.
fn cell(row: &Row) -> String {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");
    assert!(append_row(&path, "o/r", row).unwrap());
    let text = fs::read_to_string(&path).unwrap();
    let line = text
        .lines()
        .find(|l| l.contains("| #7 "))
        .unwrap_or_else(|| panic!("no row in {text}"));
    line.split('|').nth(8).unwrap().trim().to_owned()
}

#[test]
fn one_clean_round_shows_its_zero() {
    assert_eq!(cell(&row(Some(1), vec![Some(0)])), "1 (0)");
}

#[test]
fn four_rounds_show_every_count_in_order() {
    let four = row(Some(4), vec![Some(1), Some(1), Some(0), Some(0)]);
    assert_eq!(cell(&four), "4 (1, 1, 0, 0)");
}

#[test]
fn a_round_with_no_readable_count_shows_a_question_mark() {
    assert_eq!(
        cell(&row(Some(3), vec![Some(1), None, Some(0)])),
        "3 (1, ?, 0)"
    );
}

#[test]
fn the_round_count_is_the_number_of_saved_reviews_as_task_facts_counts_them() {
    // Round 1 went to a gate failure, so the record's round 3 is the second review.
    assert_eq!(cell(&row(Some(3), vec![Some(1), Some(0)])), "2 (1, 0)");
}

#[test]
fn saved_reviews_show_with_no_review_in_the_record() {
    // Review M-1: a first review with no readable count is saved, but the record gets no review.
    assert_eq!(cell(&row(None, vec![None])), "1 (?)");
    assert_eq!(cell(&row(None, vec![None, None])), "2 (?, ?)");
}

#[test]
fn no_review_round_shows_a_dash() {
    assert_eq!(cell(&row(None, Vec::new())), "-");
}

#[test]
fn the_counts_are_read_from_the_saved_review_files_in_round_order() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(
        dir.join("review-10.md"),
        "Reviewer: claude\n\nMust-fix findings: 0\n",
    )
    .unwrap();
    fs::write(
        dir.join("review-2.md"),
        "Reviewer: claude\n\nMust-fix findings: 3\r\n",
    )
    .unwrap();
    fs::write(dir.join("review-3.md"), "no count here\n").unwrap();
    fs::write(dir.join("review.md"), "Must-fix findings: 9\n").unwrap();

    assert_eq!(review::saved_must_fix(dir), [Some(3), None, Some(0)]);
}

#[test]
fn a_folder_with_no_review_files_has_no_counts() {
    let tmp = tempfile::TempDir::new().unwrap();
    assert!(review::saved_must_fix(tmp.path()).is_empty());
    assert!(review::saved_must_fix(&tmp.path().join("gone")).is_empty());
}
