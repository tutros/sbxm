//! Issue 503, answer A (a): `task rm`'s delete of a task's folder is tried again while Windows
//! reports a sharing violation (os error 32) or `PermissionDenied`, 5 tries in all, 1 s apart;
//! any other error is reported at once.

use crate::common;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play};
use sbxm::backend::FakeBackend;
use sbxm::task::finish::{plan_removal, remove_with_delete};
use sbxm::task::record;

fn worked(f: &Fixture) -> FakeBackend {
    let b = backend().with_exec_output_matching("claude", ok(CLAUDE_DONE));
    let b = b.with_exec_hook(play(
        &f.env.base_dir().join("tasks").join("issue-41"),
        "main",
        "issue-41",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    ));
    common::task_fixture::worked_task(f, &b);
    b
}

/// A delete of the task folder (`.sbxm/tasks/issue-41`) that fails with `error` on its first
/// `failures` tries, then really deletes; every other folder is deleted at once. Counts the task
/// folder's tries.
struct FlakyDelete {
    meta: PathBuf,
    failures: u32,
    error: fn() -> std::io::Error,
    tries: Cell<u32>,
}

impl FlakyDelete {
    fn new(f: &Fixture, failures: u32, error: fn() -> std::io::Error) -> Self {
        Self {
            meta: record::task_dir(&f.env.base_dir(), "issue-41"),
            failures,
            error,
            tries: Cell::new(0),
        }
    }

    fn delete(&self, folder: &Path) -> std::io::Result<()> {
        if folder == self.meta {
            self.tries.set(self.tries.get() + 1);
            if self.tries.get() <= self.failures {
                return Err((self.error)());
            }
        }
        std::fs::remove_dir_all(folder)
    }
}

fn sharing_violation() -> std::io::Error {
    std::io::Error::from_raw_os_error(32)
}

fn denied() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::PermissionDenied)
}

fn not_found() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::NotFound)
}

fn other() -> std::io::Error {
    std::io::Error::other("the disk went away")
}

fn run(
    f: &Fixture,
    b: &FakeBackend,
    flaky: &FlakyDelete,
) -> (sbxm::task::finish::RemovalReport, Vec<Duration>) {
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();
    let sleeps = RefCell::new(Vec::new());
    let report = remove_with_delete(&plan, b, &|d| sleeps.borrow_mut().push(d), &|folder| {
        flaky.delete(folder)
    });
    (report, sleeps.into_inner())
}

#[test]
fn a_task_folder_that_fails_four_times_with_a_sharing_violation_is_removed_on_the_fifth() {
    let f = fixture();
    let b = worked(&f);
    let flaky = FlakyDelete::new(&f, 4, sharing_violation);

    let (report, sleeps) = run(&f, &b, &flaky);

    assert!(report.is_clean(), "{report:?}");
    assert!(!flaky.meta.exists(), "the task folder stayed");
    assert_eq!(flaky.tries.get(), 5);
    assert_eq!(sleeps, vec![Duration::from_secs(1); 4]);
}

#[test]
fn a_task_folder_denied_four_times_is_removed_on_the_fifth() {
    let f = fixture();
    let b = worked(&f);
    let flaky = FlakyDelete::new(&f, 4, denied);

    let (report, _) = run(&f, &b, &flaky);

    assert!(report.is_clean(), "{report:?}");
    assert!(!flaky.meta.exists(), "the task folder stayed");
}

#[test]
fn a_task_folder_that_fails_five_times_stays_with_the_warning_after_exactly_five_tries() {
    let f = fixture();
    let b = worked(&f);
    let flaky = FlakyDelete::new(&f, 5, sharing_violation);

    let (report, sleeps) = run(&f, &b, &flaky);

    assert_eq!(flaky.tries.get(), 5);
    assert_eq!(sleeps.len(), 4);
    assert!(flaky.meta.exists());
    let stayed = report.stayed.join("\n");
    assert!(
        stayed.contains(&format!("{} stayed: cannot delete", flaky.meta.display())),
        "{stayed}"
    );
}

#[test]
fn another_error_is_reported_at_once_without_a_retry() {
    for error in [not_found as fn() -> std::io::Error, other] {
        let f = fixture();
        let b = worked(&f);
        let flaky = FlakyDelete::new(&f, 1, error);

        let (report, sleeps) = run(&f, &b, &flaky);

        assert_eq!(flaky.tries.get(), 1);
        assert!(sleeps.is_empty(), "{sleeps:?}");
        assert!(flaky.meta.exists());
        let stayed = report.stayed.join("\n");
        assert!(
            stayed.contains(&format!("{} stayed: cannot delete", flaky.meta.display())),
            "{stayed}"
        );
    }
}
