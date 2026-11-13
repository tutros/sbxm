//! Issue 615 (milestone 5 slice 4, decisions 317(4), (6), 321): the loop keeps up to
//! `sandbox_slots` local and `cloud_slots` cloud children running at once, each an issue task end
//! to end; its log row names the slot a task ran in.

use std::fs;

use sbxm::backend::Location;
use sbxm::task::queue::{Row, append_row};

// --- the log's Slot column ---

fn row(issue: u32, slot: Option<Location>) -> Row {
    Row {
        issue,
        title: "Fix it".into(),
        size: Some("S".into()),
        slot,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds: None,
        must_fix: None,
        outcome: "parked at working".into(),
        stops: Vec::new(),
        idle_before: None,
    }
}

#[test]
fn a_rows_slot_column_names_where_its_task_ran() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");

    append_row(&path, "o/r", &row(1, Some(Location::Local))).unwrap();
    append_row(&path, "o/r", &row(2, Some(Location::Cloud))).unwrap();
    append_row(&path, "o/r", &row(3, None)).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("| # | Issue | Size | Slot | Started | Ready (time to ready) |"),
        "{text}"
    );
    assert!(
        text.contains("| 1 | #1 Fix it | S | local | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 2 | #2 Fix it | S | cloud | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 3 | #3 Fix it | S | - | 14:59:18 | - |"),
        "{text}"
    );
}
