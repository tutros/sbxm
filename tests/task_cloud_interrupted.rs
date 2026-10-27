//! PR 322 review M-1: a cloud create cut off before it returned (sbxm killed while the cloud may
//! already have accepted it) still leaves the sandbox in `task.json`, so `task rm` removes it in
//! the cloud, then the secrets scoped to it, and only then the task's folder. A create that
//! returned an error leaves nothing of its own, so a sandbox of the same name that was there
//! before is never removed. The interruption is a create hook that panics: the process never
//! learns how the create ended. Through `FakeBackend`.

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

/// `backend`, killed in the middle of its create of `name`.
fn killed_at_create_of(backend: FakeBackend, name: &str) -> FakeBackend {
    let name = name.to_owned();
    backend.with_create_hook(move |spec| {
        if spec.name == name {
            panic!("killed during the create of {name}");
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

/// The log of an `rm` of each of `names` in the cloud, then of the secret `sbxm-fake` scoped to
/// it.
fn removed_with_secret(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .flat_map(|name| {
            [
                format!("rm {name}"),
                format!("secret-ls {name}"),
                format!("secret-rm {name} sbxm-fake"),
            ]
        })
        .collect()
}

/// A fake cloud holding the sandboxes `names`, each with the scoped secret `sbxm-fake`.
fn cloud_holding(names: &[&str]) -> FakeBackend {
    names.iter().fold(FakeBackend::default(), |backend, name| {
        backend.with_cloud_secret(name, "sbxm-fake")
    })
}

/// Boundary: killed in the start's create of the worker sandbox, before its entry could follow
/// it. `task rm` removes it in the cloud, then its secret, then the folder.
#[test]
fn rm_after_a_start_killed_in_its_worker_create_removes_the_sandbox_its_secrets_then_the_folder() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = killed_at_create_of(cloud_backend(&root, &id, &[CLEAN]), &worker);
    killed(|| {
        task_run(&f, spec.clone(), Some(Location::Cloud), &backend);
    });
    assert!(meta(&f, &id).join("task.json").is_file());
    let cloud = cloud_holding(&[&worker]);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert_eq!(cloud.log(), removed_with_secret(&[&worker]), "{out}");
    assert_eq!(cloud.locations(), vec![Location::Cloud]);
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary: killed in the create of the first reviewer sandbox, the worker's still open. Both
/// go, each with its secret, then the folder.
#[test]
fn rm_after_a_run_killed_in_its_reviewer_create_removes_both_sandboxes_then_the_folder() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = killed_at_create_of(cloud_backend(&root, &id, &[CLEAN]), &reviewer);
    killed(|| {
        task_run(&f, spec.clone(), Some(Location::Cloud), &backend);
    });
    let cloud = cloud_holding(&[&worker, &reviewer]);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert_eq!(
        cloud.log(),
        removed_with_secret(&[&worker, &reviewer]),
        "{out}"
    );
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary: a task stopped `ready` (its worker's entry stamped removed) is resumed with
/// `--rounds 1` and killed in the create of the fix round's rebuilt worker sandbox, whose name
/// the older, removed entry already has. `task rm` still removes it, then its secret.
#[test]
fn rm_after_a_resume_killed_in_its_rebuilt_fix_create_removes_the_sandbox_named_by_a_removed_entry()
{
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    let task = task_record(&f, &id);
    assert!(
        task.sandboxes
            .iter()
            .any(|e| e.name == worker && e.removed_at.is_some())
    );
    assert!(!task.has_open_sandbox(&worker));
    let backend = killed_at_create_of(cloud_backend_listing(&root, &id, &[CLEAN], false), &worker);
    killed(|| {
        let _ = sbxm::commands::task_resume::run(
            &f.env.config_dir(),
            &sbxm::commands::task_resume::Options {
                repo_root: f.env.tmp.path().join("target-repo"),
                target: sbxm::commands::task_resume::Target::Spec(spec.clone()),
                rounds: Some(1),
                disk: Default::default(),
                identity: Some(identity()),
            },
            &backend,
            &Gone,
            &sbxm::task::gates::FakeHostRunner::default(),
            &mut Vec::new(),
            &mut Vec::new(),
        );
    });
    let cloud = cloud_holding(&[&worker]);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert_eq!(cloud.log(), removed_with_secret(&[&worker]), "{out}");
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The guard kept: a reviewer create that returned an error made no sandbox of the task's, so a
/// sandbox of that name the cloud already had is not the task's to remove, then or at `task rm`.
#[test]
fn a_reviewer_create_that_failed_leaves_a_same_named_sandbox_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend(&root, &id, &[CLEAN]).with_failing_create_for(&reviewer);

    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &backend);

    assert!(ran.result.is_err(), "{}", ran.out);
    assert!(
        !backend.removes().contains(&reviewer),
        "{:?}",
        backend.log()
    );
    let task = task_record(&f, &id);
    assert!(!task.has_open_sandbox(&reviewer), "{:?}", task.sandboxes);
    assert!(
        task.sandboxes
            .iter()
            .all(|e| e.role != SandboxRole::Reviewer),
        "{:?}",
        task.sandboxes
    );
    let cloud = cloud_holding(&[&worker, &reviewer]);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(!cloud.removes().contains(&reviewer), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The guard kept for a rebuilt worker: a fix round's create that returned an error leaves no
/// open entry, so `task rm` leaves a sandbox of the worker's name alone.
#[test]
fn a_rebuilt_fix_create_that_failed_leaves_a_same_named_sandbox_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    let entries = task_record(&f, &id).sandboxes.len();
    let backend =
        cloud_backend_listing(&root, &id, &[CLEAN], false).with_failing_create_for(&worker);
    let result = sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec.clone()),
            rounds: Some(1),
            disk: Default::default(),
            identity: Some(identity()),
        },
        &backend,
        &Gone,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert!(result.is_err());
    assert!(!backend.removes().contains(&worker), "{:?}", backend.log());
    assert_eq!(task_record(&f, &id).sandboxes.len(), entries);
    let cloud = cloud_holding(&[&worker]);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.removes().is_empty(), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}
