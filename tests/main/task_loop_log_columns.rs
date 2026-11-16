//! Issue 696: the loop log has no "User interventions" column (it said `unknown` on every row),
//! and logs written with the old 12-column header still take rows and are still posted late.

use crate::common;

use std::fs;

use common::task_fixture::open_issue;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::loop_log;
use sbxm::task::queue::{Row, append_row};

const REPO: &str = "o/r";

/// The header of a log written before issue 696, with its 12 columns.
const OLD_LOG: &str = "# Loop log (decision 245(h))

Repository: o/r

Times UTC. Written by `sbxm task loop`, one row per task; `unknown` where not measured.

| # | Issue | Size | Slot | Started | Ready (time to ready) | Worker / reviewer s | Rounds \
(must-fix) | Outcome | Stops (cause, duration) | Idle before | User interventions |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | #290 Old | S | - | 14:00:00 | - | - | - | parked at working | none | - | unknown |
";

fn row(issue: u32) -> Row {
    Row {
        issue,
        title: "Fix it".into(),
        size: Some("S".into()),
        slot: None,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds: None,
        must_fix: Vec::new(),
        outcome: "PR #293, clean".into(),
        stops: Vec::new(),
        idle_before: Some(190),
    }
}

/// The cells of a table line, outer pipes dropped.
fn cells(line: &str) -> Vec<&str> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(str::trim).collect()
}

fn line_starting<'a>(text: &'a str, start: &str) -> &'a str {
    text.lines()
        .find(|l| l.starts_with(start))
        .unwrap_or_else(|| panic!("no line starting {start:?} in {text}"))
}

#[test]
fn task_loop_log_columns_a_new_log_has_11_columns_and_no_user_interventions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");

    assert!(append_row(&path, REPO, &row(291)).unwrap());

    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("User interventions"), "{text}");
    let header = cells(line_starting(&text, "| # |"));
    assert_eq!(header.len(), 11, "{text}");
    assert_eq!(header.last(), Some(&"Idle before"), "{text}");
    assert_eq!(cells(line_starting(&text, "|---")).len(), 11, "{text}");
}

#[test]
fn task_loop_log_columns_a_row_has_11_cells_the_last_idle_before() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");

    append_row(&path, REPO, &row(291)).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    let row = cells(line_starting(&text, "| 1 |"));
    assert_eq!(row.len(), 11, "{text}");
    assert_eq!(row.last(), Some(&"3m10s"), "{text}");
}

#[test]
fn task_loop_log_columns_a_row_appended_to_an_old_12_column_log_leaves_its_rows_alone() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");
    fs::write(&path, OLD_LOG).unwrap();

    assert!(append_row(&path, REPO, &row(291)).unwrap());
    assert!(
        !append_row(&path, REPO, &row(290)).unwrap(),
        "already logged"
    );

    let text = fs::read_to_string(&path).unwrap();
    let added = text.strip_prefix(OLD_LOG).expect(&text);
    assert!(added.starts_with("| 2 | #291 Fix it | S |"), "{text}");
    assert_eq!(cells(added.trim_end()).len(), 11, "{text}");
    assert_eq!(added.lines().count(), 1, "{text}");
}

#[test]
fn task_loop_log_columns_an_old_12_column_log_is_still_posted_late() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path();
    let started = 1_762_596_000; // 2025-11-08T10:00:00Z
    let earlier = loop_log::path(base, started - 86_400);
    fs::create_dir_all(earlier.parent().unwrap()).unwrap();
    fs::write(&earlier, OLD_LOG).unwrap();
    append_row(&earlier, REPO, &row(291)).unwrap();
    let gh = FakeGitHub::default().with_open_issues(vec![open_issue(40, &["loop-log"], "")]);

    let late = loop_log::post_late(&gh, REPO, base, &loop_log::path(base, started), started);

    assert!(late.failed.is_empty(), "{:?}", late.failed);
    assert!(late.skipped.is_empty(), "{:?}", late.skipped);
    let posted: Vec<String> = gh
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueComment(_, 40, body) => Some(body),
            _ => None,
        })
        .collect();
    assert_eq!(posted.len(), 1, "{posted:?}");
    let log = fs::read_to_string(&earlier).unwrap();
    assert!(posted[0].ends_with(&log), "{}", posted[0]);
    assert!(loop_log::marker(&earlier).exists());
}
