//! Issue 616 (decision 322): a command the loop started as its child records the loop in the
//! records it takes. This test binary is such a child: every test sets the same loop for the
//! whole process, as `main` does from the loop's environment variable, so no test here may
//! expect a task without one.

use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Status};

/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;
const LOOP: u32 = 77;
const CHILD: u32 = 88;

fn started_by_the_loop() {
    record::started_by_loop(Process::new(LOOP, START));
}

fn new_record(process: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 3,
            repo: "o/r",
            title: "Fix 3",
            base: "main",
            branch: "sbxm/issue-3",
            config_hash: "h",
            id: None,
        },
        START,
        Process::new(process, START),
    )
}

#[test]
fn a_task_the_child_starts_records_the_loop() {
    started_by_the_loop();

    let rec = new_record(CHILD);

    assert_eq!(rec.process, Some(Process::new(CHILD, START)));
    assert_eq!(rec.loop_owner, Some(Process::new(LOOP, START)));
}

#[test]
fn a_task_the_child_resumes_records_the_loop() {
    started_by_the_loop();
    let mut rec = new_record(1);
    rec.loop_owner = None;
    rec.stage = Stage::Working;
    rec.status = Status::Failed;

    rec.rerun(START, Process::new(CHILD, START)).unwrap();

    assert_eq!(rec.process, Some(Process::new(CHILD, START)));
    assert_eq!(rec.loop_owner, Some(Process::new(LOOP, START)));
}
