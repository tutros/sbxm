//! Issue 486: a `priority:high` label puts an issue first in the loop's queue (still size S, M,
//! L, then oldest first among the labeled ones), and `sbxm status`'s queue shows it first.

use crate::common;

use std::fs;
use std::path::Path;

use common::Env as TestEnv;
use common::task_fixture::open_issue;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, IssueText};
use sbxm::task::queue::{largest_first, lock_path, queue_order};
use sbxm::task::record::{Process, ProcessProbe};

#[test]
fn task_loop_priority_high_issues_come_first_by_size_then_oldest() {
    let open = vec![
        open_issue(9, &["ready-for-worker", "size:M"], ""),
        open_issue(4, &["ready-for-worker", "size:L", "priority:high"], ""),
        open_issue(7, &["ready-for-worker", "size:S"], ""),
        open_issue(6, &["ready-for-worker", "size:M", "priority:high"], ""),
        open_issue(5, &["ready-for-worker", "size:M", "priority:high"], ""),
        open_issue(8, &["ready-for-worker", "priority:high"], ""),
        open_issue(1, &["ready-for-worker", "size:S"], ""),
    ];
    // High first (M oldest first, then L, then unsized), then the rest as before.
    assert_eq!(queue_order(&open), [5, 6, 4, 8, 1, 7, 9]);
}

#[test]
fn task_loop_priority_high_without_the_ready_label_is_not_queued() {
    let open = vec![
        open_issue(3, &["size:S", "priority:high"], ""),
        open_issue(9, &["ready-for-worker", "size:L"], ""),
    ];
    assert_eq!(queue_order(&open), [9]);
}

#[test]
fn task_loop_priority_other_priority_labels_do_not_move_an_issue() {
    let open = vec![
        open_issue(9, &["ready-for-worker", "size:S", "priority:low"], ""),
        open_issue(2, &["ready-for-worker", "size:S"], ""),
    ];
    assert_eq!(queue_order(&open), [2, 9]);
}

#[test]
fn task_loop_priority_high_comes_first_for_a_cloud_slot_too() {
    let open = vec![
        open_issue(9, &["ready-for-worker", "size:L"], ""),
        open_issue(4, &["ready-for-worker", "size:S", "priority:high"], ""),
        open_issue(7, &["ready-for-worker", "size:M", "priority:high"], ""),
        open_issue(2, &["ready-for-worker", "size:S"], ""),
    ];
    // Largest first within the high ones, then within the rest.
    assert_eq!(largest_first(&open), [7, 4, 9, 2]);
}

struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(1)
    }
}

fn status_frame(base: &Path, gh: &FakeGitHub) -> String {
    let lock = lock_path(base);
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, serde_json::to_string(&Process::new(1, 1)).unwrap()).unwrap();
    let env = Env {
        placement: Default::default(),
        now: Box::new(|| 1_790_000_000),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(std::path::PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    };
    let source = LoopSource {
        github: gh,
        repo: Ok("o/r".into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        &Alive,
        &env,
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn task_loop_priority_status_shows_the_high_issues_first() {
    let base = TestEnv::new();
    let issues = vec![
        Issue {
            title: "Four".into(),
            ..open_issue(4, &["ready-for-worker", "size:S"], "")
        },
        Issue {
            title: "Nine".into(),
            ..open_issue(9, &["ready-for-worker", "size:M", "priority:high"], "")
        },
    ];
    let mut gh = FakeGitHub::default();
    for i in &issues {
        gh = gh.with_issue_text(IssueText {
            number: i.number,
            title: i.title.clone(),
            state: "OPEN".into(),
            text: String::new(),
        });
    }
    let gh = gh.with_open_issues(issues);

    let text = status_frame(&base.base_dir(), &gh);

    assert!(
        text.contains("  queue: 2 issues\n    #9: Nine (size M)\n    #4: Four (size S)\n"),
        "{text}"
    );
}
