//! Milestone 4 slice 6 (`sdlc/milestone-4.md`, work-order item 6; P5 (d), P6; decisions 253,
//! 259, 263, 265): `task finish` stays host-side, and then removes any cloud sandbox of the task
//! whose `sandboxes` entry still has no `removed_at` (a removal at the task's stop that failed),
//! retried, then the secrets scoped to it. A removal that still fails is reported and doesn't
//! stop the finish. The record decides, whatever the config says. Through `FakeBackend`.

mod common;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use common::cloud_task::{CLEAN, cloud_backend, spec, task_record, task_run, worker_sandbox};
use common::task_fixture::{Fixture, Probe, fixture_with, ready_spec_task};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_finish::{Options, Target, run_with_sleep};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Stage};

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

const UNAVAILABLE: &str = "error: sandbox temporarily unavailable, try again";

/// A cloud spec task run to `ready`, the config saying `local` from then on, and its worker
/// sandbox's entry left open, as a removal at its stop that failed leaves it.
fn ready_cloud_task_with_its_worker_open() -> (Fixture, PathBuf, String) {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[CLEAN]),
    )
    .result
    .unwrap();
    let mut task = task_record(&f, &id);
    let worker = worker_sandbox(&id);
    for entry in task.sandboxes.iter_mut().filter(|e| e.name == worker) {
        entry.removed_at = None;
    }
    record::write(&record::task_dir(&f.env.base_dir(), &id), &task).unwrap();
    (f, spec, id)
}

/// `task finish --spec FILE` (the `local` sink) through `backend`, recording each wait.
fn finish(
    f: &Fixture,
    spec: PathBuf,
    backend: &FakeBackend,
) -> (anyhow::Result<()>, String, Vec<u64>) {
    let waits = Mutex::new(Vec::new());
    let mut out = Vec::new();
    let result = run_with_sleep(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        backend,
        &FakeGitHub::default(),
        &Probe,
        &mut out,
        &|wait: Duration| waits.lock().unwrap().push(wait.as_secs()),
    );
    (
        result,
        String::from_utf8(out).unwrap(),
        waits.into_inner().unwrap(),
    )
}

/// A cloud task whose worker sandbox's removal failed at the stop: `finish` removes it in the
/// cloud, then its scoped secrets, and stamps its entry; the config's `local` changes nothing.
#[test]
fn finish_removes_a_cloud_sandbox_still_listed_then_its_secrets() {
    let (f, spec, id) = ready_cloud_task_with_its_worker_open();
    let worker = worker_sandbox(&id);
    let backend = FakeBackend::default().with_cloud_secret(&worker, "sbxm-fake");

    let (result, out, waits) = finish(&f, spec, &backend);

    result.unwrap();
    assert_eq!(
        backend.log(),
        vec![
            format!("rm {worker}"),
            format!("secret-ls {worker}"),
            format!("secret-rm {worker} sbxm-fake"),
        ],
        "{out}"
    );
    assert_eq!(backend.locations(), vec![Location::Cloud]);
    assert!(waits.is_empty());
    let task = task_record(&f, &id);
    assert_eq!(task.stage, Stage::Finished);
    assert!(
        task.sandboxes.iter().all(|e| e.removed_at.is_some()),
        "{:?}",
        task.sandboxes
    );
    assert!(
        out.contains(&format!("cloud sandbox {worker} removed")),
        "{out}"
    );
}

/// A removal that still fails after its retries is reported, naming the manual command, and the
/// finish itself still succeeds; the entry stays open.
#[test]
fn finish_reports_a_cloud_sandbox_it_cannot_remove_and_still_finishes() {
    let (f, spec, id) = ready_cloud_task_with_its_worker_open();
    let worker = worker_sandbox(&id);
    let backend = FakeBackend::default().with_remove_errors(&[UNAVAILABLE; 4]);

    let (result, out, waits) = finish(&f, spec, &backend);

    result.unwrap();
    assert_eq!(backend.removes(), vec![worker.clone(); 4]);
    assert_eq!(waits, vec![5, 10, 20]);
    assert!(
        out.contains(&format!("sbx --cloud rm {worker} -f")),
        "{out}"
    );
    let task = task_record(&f, &id);
    assert_eq!(task.stage, Stage::Finished);
    assert!(task.has_open_sandbox(&worker));
}

/// The reverse: a task recorded `local` while the config says `cloud` finishes with no backend
/// call at all.
#[test]
fn finish_of_a_local_task_under_a_cloud_config_calls_no_backend() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = ready_spec_task(&f);
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();
    let backend = FakeBackend::default();

    let (result, out, _) = finish(&f, spec, &backend);

    result.unwrap();
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(backend.locations().is_empty(), "{out}");
    assert_eq!(task_record(&f, &id).stage, Stage::Finished);
}
