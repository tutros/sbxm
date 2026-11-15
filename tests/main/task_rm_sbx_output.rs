//! Issue 114, review M-1: a `task rm` whose sandbox won't go keeps the task's folder, and the lines
//! its `sbx rm` printed are in that task's `run.log`, local or cloud. `FakeBackend` prints them.

use crate::common;

use std::fs;
use std::path::Path;

use common::cloud_task::{CLEAN, cloud_backend, spec, task_record, task_run, worker_sandbox};
use common::task_fixture::{
    CLAUDE_DONE, Play, Probe, backend, fixture, fixture_with, ok, play, worked_task,
};
use sbxm::backend::{FakeBackend, Location, SandboxInfo};
use sbxm::task::finish::{plan_removal, remove, remove_with_sleep};
use sbxm::task::record;

const UNAVAILABLE: &str = "error: sandbox temporarily unavailable, try again";

fn run_log(meta: &Path) -> String {
    fs::read_to_string(meta.join("run.log")).unwrap_or_default()
}

#[test]
fn a_local_task_rm_that_fails_logs_the_sbx_rm_lines_in_the_kept_task_log() {
    let f = fixture();
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let prepared = worked_task(&f, &b);
    let sandbox = prepared.record.worker.as_ref().unwrap().sandbox.clone();
    let stuck = FakeBackend::failing_remove()
        .and_sandboxes(vec![SandboxInfo {
            name: sandbox.clone(),
            agent: "claude".into(),
            status: "running".into(),
        }])
        .with_remove_output(&["{name}: removing"]);
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &stuck);

    assert!(!report.is_clean(), "{report:?}");
    let log = run_log(&prepared.meta);
    assert!(log.contains(&format!("{sandbox}: removing")), "{log}");
}

#[test]
fn a_cloud_task_rm_that_fails_logs_each_tries_sbx_rm_lines_in_the_kept_task_log() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    );
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec,
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[CLEAN]),
    );
    ran.result.unwrap();
    // Its worker's removal failed at the stop: the entry is still open.
    let worker = worker_sandbox(&id);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    for entry in task.sandboxes.iter_mut().filter(|e| e.name == worker) {
        entry.removed_at = None;
    }
    record::write(&meta, &task).unwrap();
    let stuck = FakeBackend::default()
        .with_remove_errors(&[UNAVAILABLE; 4])
        .with_remove_output(&["{name}: removing"]);
    let plan = plan_removal(&f.env.base_dir(), &id, &Probe).unwrap();

    let report = remove_with_sleep(&plan, &stuck, &|_| {});

    assert!(!report.is_clean(), "{report:?}");
    let log = run_log(&meta);
    let line = format!("{worker}: removing");
    assert_eq!(log.matches(&line).count(), 4, "{log}");
}
