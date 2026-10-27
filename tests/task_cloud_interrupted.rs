//! PR 322 review M-1, round 2: a cloud create cut off before `sbxm` heard back. `task.json`
//! gets a sandbox's lifetime entry only once its create returned (decision 265), so a run killed
//! after the cloud accepted a create leaves no entry for it; `task rm` (with `--restart` and
//! `rm --merged`, which plan the same way) still removes it, then the secrets scoped to it, then
//! the folder, because the task is interrupted and the record names the sandbox. A create that
//! returned an error leaves the task stopped, not interrupted, so a sandbox of that name the
//! cloud already had is never removed. Each test runs and removes the task through one fake
//! cloud ([`FakeBackend::with_live_cloud`]) that knows whether the cut-off create was accepted.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use common::cloud_task::{
    CLEAN, ONE, cloud_backend, cloud_backend_listing, identity, reviewer_sandbox, spec,
    task_record, task_run, worker_sandbox,
};
use common::task_fixture::{Fixture, fixture_with};
use sbxm::backend::{FakeBackend, Location};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, ProcessProbe, SandboxRole};

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// The process that ran the task is gone: it was killed.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Never asked: every removal here passes `--yes`.
struct Never;

impl Confirm for Never {
    fn is_interactive(&self) -> bool {
        false
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        panic!("not asked with --yes")
    }
}

/// `backend`, killed in its create of `name` before the cloud had it.
fn killed_at_create_of(backend: FakeBackend, name: &str) -> FakeBackend {
    let name = name.to_owned();
    backend.with_create_hook(move |spec| {
        if spec.name == name {
            panic!("killed before the cloud accepted {name}");
        }
    })
}

/// Runs `body`, which is killed (panics) in a create.
fn killed(body: impl FnOnce()) {
    let result = catch_unwind(AssertUnwindSafe(body));
    assert!(result.is_err(), "the run was not killed");
}

/// `task rm --spec FILE --yes --force` through `backend`, the task's process gone.
fn rm(f: &Fixture, spec: &Path, backend: &FakeBackend) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = sbxm::commands::task_rm::run_with_sleep(
        &f.env.config_dir(),
        &sbxm::commands::task_rm::Options {
            target: sbxm::commands::task_rm::Target::Spec(spec.to_path_buf()),
            yes: true,
            force: true,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
        },
        backend,
        &FakeGitHub::default(),
        &Gone,
        &Never,
        &mut out,
        &|_| {},
    );
    (result, String::from_utf8(out).unwrap())
}

fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

/// `backend` as one cloud that already holds the sandboxes `before`: a create it accepts adds
/// the sandbox, a removal takes it away, and each of `scoped` has the secret `sbxm-fake` scoped
/// to it, standing in for the one set before its create.
fn one_cloud(backend: FakeBackend, before: &[&str], scoped: &[&str]) -> FakeBackend {
    scoped
        .iter()
        .fold(backend.with_live_cloud(before), |b, name| {
            b.with_cloud_secret(name, "sbxm-fake")
        })
}

/// `backend`, killed once the cloud accepted its create of `name`, before `sbxm` heard back.
fn killed_after_accepting(backend: FakeBackend, name: &str) -> FakeBackend {
    let name = name.to_owned();
    backend.with_accepted_create_hook(move |spec| {
        if spec.name == name {
            panic!("killed after the cloud accepted {name}");
        }
    })
}

/// The secrets `backend`'s cloud still scopes to `name`.
fn secrets_of(backend: &FakeBackend, name: &str) -> Vec<String> {
    backend
        .cloud_secrets()
        .into_iter()
        .filter(|(scope, _)| scope == name)
        .map(|(_, secret)| secret)
        .collect()
}

/// `task resume --spec FILE --rounds 1` through `backend`, the task's process gone.
fn resume_one_round(f: &Fixture, spec: &Path, backend: &FakeBackend) -> anyhow::Result<()> {
    sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec.to_path_buf()),
            rounds: Some(1),
            disk: Default::default(),
            identity: Some(identity()),
        },
        backend,
        &Gone,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

/// A cloud spec task run to `ready` with its one fix round used; its worker sandbox, and its
/// entry, went at that stop.
fn stopped_ready(f: &Fixture, spec: &Path, id: &str) {
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        f,
        spec.to_path_buf(),
        Some(Location::Cloud),
        &cloud_backend(&root, id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    let worker = worker_sandbox(id);
    let task = task_record(f, id);
    assert!(
        task.sandboxes
            .iter()
            .any(|e| e.name == worker && e.removed_at.is_some())
    );
    assert!(!task.has_open_sandbox(&worker));
}

/// Boundary, after acceptance: killed once the cloud had the start's worker sandbox. It has no
/// entry, and `task rm` removes it and its secret all the same, then the folder.
#[test]
fn rm_after_a_start_killed_once_its_worker_was_accepted_removes_it_its_secret_then_the_folder() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_after_accepting(cloud_backend(&root, &id, &[CLEAN]), &worker),
        &[],
        &[&worker],
    );
    killed(|| {
        task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);
    });
    assert_eq!(cloud.live_cloud(), vec![worker.clone()]);
    assert!(task_record(&f, &id).sandboxes.is_empty());

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.live_cloud().is_empty(), "{:?}", cloud.log());
    assert!(secrets_of(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary, after acceptance: killed once the cloud had the first reviewer sandbox, the
/// worker's still open. Both go, each with its secret, then the folder.
#[test]
fn rm_after_a_run_killed_once_its_reviewer_was_accepted_removes_both_then_the_folder() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_after_accepting(cloud_backend(&root, &id, &[CLEAN]), &reviewer),
        &[],
        &[&worker, &reviewer],
    );
    killed(|| {
        task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);
    });
    assert_eq!(cloud.live_cloud(), vec![worker.clone(), reviewer.clone()]);
    assert!(!task_record(&f, &id).has_open_sandbox(&reviewer));

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.live_cloud().is_empty(), "{:?}", cloud.log());
    assert!(secrets_of(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert!(
        secrets_of(&cloud, &reviewer).is_empty(),
        "{:?}",
        cloud.log()
    );
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary, after acceptance: a task stopped `ready` (its worker's entry stamped removed) is
/// resumed with `--rounds 1` and killed once the cloud had the fix round's rebuilt worker, whose
/// name the older, removed entry has. `task rm` still removes it and its secret.
#[test]
fn rm_after_a_resume_killed_once_its_rebuilt_fix_was_accepted_removes_it_despite_a_removed_entry() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    stopped_ready(&f, &spec, &id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_after_accepting(cloud_backend_listing(&root, &id, &[CLEAN], false), &worker),
        &[],
        &[&worker],
    );
    killed(|| {
        let _ = resume_one_round(&f, &spec, &cloud);
    });
    assert_eq!(cloud.live_cloud(), vec![worker.clone()]);
    assert!(!task_record(&f, &id).has_open_sandbox(&worker));

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.live_cloud().is_empty(), "{:?}", cloud.log());
    assert!(secrets_of(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary, before acceptance: killed in the start's worker create before the cloud had it.
/// No entry claims the sandbox; `task rm` finds none in the cloud, removes the secret the start
/// scoped to its name, then the folder.
#[test]
fn rm_after_a_start_killed_before_its_worker_was_accepted_claims_no_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_at_create_of(cloud_backend(&root, &id, &[CLEAN]), &worker),
        &[],
        &[&worker],
    );
    killed(|| {
        task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);
    });
    assert!(cloud.live_cloud().is_empty());
    assert!(task_record(&f, &id).sandboxes.is_empty());

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.live_cloud().is_empty(), "{:?}", cloud.log());
    assert!(secrets_of(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The guard kept: a reviewer create that returned an error (the cloud already had a sandbox of
/// that name) made nothing of the task's and stopped it, so that sandbox and its secret are
/// left alone, then and at `task rm`.
#[test]
fn a_reviewer_create_that_failed_leaves_a_same_named_sandbox_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        cloud_backend(&root, &id, &[CLEAN]).with_failing_create_for(&reviewer),
        &[&reviewer],
        &[&reviewer],
    );

    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);

    assert!(ran.result.is_err(), "{}", ran.out);
    let task = task_record(&f, &id);
    assert!(
        task.sandboxes
            .iter()
            .all(|e| e.role != SandboxRole::Reviewer),
        "{:?}",
        task.sandboxes
    );

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(!cloud.removes().contains(&reviewer), "{:?}", cloud.log());
    assert_eq!(cloud.live_cloud(), vec![reviewer.clone()]);
    assert_eq!(secrets_of(&cloud, &reviewer), vec!["sbxm-fake".to_owned()]);
    assert!(!cloud.live_cloud().contains(&worker));
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The guard kept for a rebuilt worker: a fix round's create that returned an error leaves no
/// open entry and a stopped task, so `task rm` leaves a sandbox of the worker's name alone.
#[test]
fn a_rebuilt_fix_create_that_failed_leaves_a_same_named_sandbox_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    stopped_ready(&f, &spec, &id);
    let entries = task_record(&f, &id).sandboxes.len();
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        cloud_backend_listing(&root, &id, &[CLEAN], false).with_failing_create_for(&worker),
        &[&worker],
        &[&worker],
    );

    assert!(resume_one_round(&f, &spec, &cloud).is_err());
    assert_eq!(task_record(&f, &id).sandboxes.len(), entries);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(
        cloud.removes().iter().all(|n| n != &worker),
        "{:?}",
        cloud.log()
    );
    assert_eq!(cloud.live_cloud(), vec![worker.clone()]);
    assert_eq!(secrets_of(&cloud, &worker), vec!["sbxm-fake".to_owned()]);
    assert!(!meta(&f, &id).exists(), "{out}");
}
