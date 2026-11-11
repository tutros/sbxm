//! Issue 579: a task whose `lock.json` is held by a live process is `running` in `task status`
//! and `sbxm status`, even while `task.json`'s `process` is still a dead earlier one (a resume
//! takes the lock before it records its own process).

use std::fs;
use std::path::Path;

use sbxm::commands::task_status;
use sbxm::status::tasks;
use sbxm::task::lock;
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage};

const T0: u64 = 1_790_000_000;
const DEAD: u32 = 1;
const LIVE: u32 = 2;

/// Only `LIVE` runs, started at `T0`.
struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (pid == LIVE).then_some(T0)
    }
}

/// A `working` task recorded as run by the process `pid`.
fn save_working(base: &Path, pid: u32) -> String {
    let mut record = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(pid, T0),
    );
    record
        .advance(Stage::Working, T0, Process::new(pid, T0))
        .unwrap();
    let dir = record::task_dir(base, &record.id);
    record::write(&dir, &record).unwrap();
    record.id
}

fn hold_lock(base: &Path, id: &str, pid: u32) {
    let process = serde_json::to_value(Process::new(pid, T0)).unwrap();
    let mut holder = process;
    holder["token"] = "t".into();
    fs::write(
        record::task_dir(base, id).join(lock::FILE_NAME),
        holder.to_string(),
    )
    .unwrap();
}

fn text_status(base: &Path) -> String {
    task_status::render(base, None, false, &Probe).unwrap().0
}

fn row_status(base: &Path) -> String {
    tasks::rows(base, &Probe).unwrap().remove(0).status
}

#[test]
fn a_dead_process_with_a_live_lock_is_running() {
    let base = tempfile::tempdir().unwrap();
    let id = save_working(base.path(), DEAD);
    hold_lock(base.path(), &id, LIVE);

    let text = text_status(base.path());
    assert!(
        text.contains("running") && !text.contains("interrupted"),
        "{text}"
    );
    assert_eq!(row_status(base.path()), "running");
    let json = task_status::render(base.path(), None, true, &Probe)
        .unwrap()
        .0;
    assert!(json.contains("\"interrupted\": false"), "{json}");
}

#[test]
fn a_dead_process_with_a_dead_lock_is_interrupted() {
    let base = tempfile::tempdir().unwrap();
    let id = save_working(base.path(), DEAD);
    hold_lock(base.path(), &id, DEAD);

    assert!(text_status(base.path()).contains("interrupted"));
    assert_eq!(row_status(base.path()), "interrupted");
}

#[test]
fn a_dead_process_with_no_lock_is_interrupted() {
    let base = tempfile::tempdir().unwrap();
    save_working(base.path(), DEAD);

    assert!(text_status(base.path()).contains("interrupted"));
    assert_eq!(row_status(base.path()), "interrupted");
}

#[test]
fn a_live_process_with_no_lock_is_running() {
    let base = tempfile::tempdir().unwrap();
    save_working(base.path(), LIVE);

    let text = text_status(base.path());
    assert!(
        text.contains("running") && !text.contains("interrupted"),
        "{text}"
    );
    assert_eq!(row_status(base.path()), "running");
}
