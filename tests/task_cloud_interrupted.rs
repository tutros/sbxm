//! PR 322 review M-1, rounds 2 and 3: a cloud create cut off before `sbxm` heard back, and a
//! cloud that already has a sandbox of the task's name. `task.json` gets a sandbox's lifetime
//! entry only once its create returned (decision 265), and holds the claim of a create from the
//! moment the name was found free in `sbx --cloud ls` until the create returned; `task rm`
//! (with `--restart` and `rm --merged`, which plan the same way) removes what the record holds,
//! entries and claim, then the secrets scoped to them, then the folder. A name the cloud already
//! had is refused before any secret is set on it or a create asked for, so a sandbox the task
//! didn't make is never removed. Each test runs and removes the task through one fake cloud
//! ([`FakeBackend::with_live_cloud`]) that lists what it holds and knows whether a cut-off create
//! was accepted.

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

/// The calls of `backend` that touched the sandbox `name` or anything scoped to it: its
/// create, removal, and each secret set, listed or removed under its name.
fn touches(backend: &FakeBackend, name: &str) -> Vec<String> {
    backend
        .log()
        .into_iter()
        .filter(|line| {
            ["create ", "rm ", "secret-set ", "secret-ls ", "secret-rm "]
                .iter()
                .any(|call| line.starts_with(call))
                && line.contains(name)
        })
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
/// No entry, but the claim of the create the run never heard back from, its name found free;
/// `task rm` removes by that name (one the cloud never made counts as removed), the secret the
/// start scoped to it, then the folder.
#[test]
fn rm_after_a_start_killed_before_its_worker_was_accepted_removes_by_its_claim() {
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
    let task = task_record(&f, &id);
    assert!(task.sandboxes.is_empty());
    assert_eq!(
        task.creating.as_ref().map(|c| c.name.clone()),
        Some(worker.clone())
    );

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(cloud.removes().contains(&worker), "{:?}", cloud.log());
    assert!(cloud.live_cloud().is_empty(), "{:?}", cloud.log());
    assert!(secrets_of(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The guard, moved before anything touches the name (review round 3, M-1): a review whose
/// reviewer name the cloud already has is refused before the reviewer's scoped secrets are set
/// and before its create, since the task can't tell that sandbox from its own, so a kill can't
/// come in a create that collides; the sandbox and its secret are left alone, then and at
/// `task rm`.
#[test]
fn a_review_whose_reviewer_name_the_cloud_already_has_is_refused_and_leaves_it_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_at_create_of(cloud_backend(&root, &id, &[CLEAN]), &reviewer),
        &[&reviewer],
        &[&reviewer],
    );

    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains(&format!("cloud sandbox {reviewer} already exists")),
        "{message}"
    );
    assert!(
        message.contains(&format!("sbx --cloud rm {reviewer} -f")),
        "{message}"
    );
    assert!(touches(&cloud, &reviewer).is_empty(), "{:?}", cloud.log());
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
    assert!(touches(&cloud, &reviewer).is_empty(), "{:?}", cloud.log());
    assert_eq!(cloud.live_cloud(), vec![reviewer.clone()]);
    assert_eq!(secrets_of(&cloud, &reviewer), vec!["sbxm-fake".to_owned()]);
    assert!(!cloud.live_cloud().contains(&worker));
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// Boundary, before acceptance, the cloud already holding the worker's name (review round 3,
/// M-1): the start is refused before it sets a secret on that name or asks for the create, so
/// that sandbox and its secret are left alone and no task is left to remove.
#[test]
fn a_start_whose_worker_name_the_cloud_already_has_is_refused_before_its_secrets_and_create() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_at_create_of(cloud_backend(&root, &id, &[CLEAN]), &worker),
        &[&worker],
        &[&worker],
    );

    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &cloud);

    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains(&format!("cloud sandbox {worker} already exists")),
        "{message}"
    );
    assert!(touches(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert_eq!(cloud.live_cloud(), vec![worker.clone()]);
    assert_eq!(secrets_of(&cloud, &worker), vec!["sbxm-fake".to_owned()]);
    assert!(!meta(&f, &id).exists(), "{}", ran.out);
}

/// The guard for a rebuilt worker (review round 3, M-1): a task stopped `ready` (its worker's
/// entry stamped removed) resumed while the cloud has a sandbox of the worker's name again is
/// refused before any secret is set on that name and before its create, since the record holds
/// no such sandbox; `task rm` leaves it and its secret alone.
#[test]
fn a_resume_whose_rebuilt_fix_name_the_cloud_already_has_is_refused_and_leaves_it_to_rm() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    stopped_ready(&f, &spec, &id);
    let entries = task_record(&f, &id).sandboxes.len();
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        cloud_backend_listing(&root, &id, &[CLEAN], false),
        &[&worker],
        &[&worker],
    );

    let message = format!("{:#}", resume_one_round(&f, &spec, &cloud).unwrap_err());

    assert!(
        message.contains(&format!("cloud sandbox {worker} already exists")),
        "{message}"
    );
    assert!(touches(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert_eq!(task_record(&f, &id).sandboxes.len(), entries);

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(touches(&cloud, &worker).is_empty(), "{:?}", cloud.log());
    assert_eq!(cloud.live_cloud(), vec![worker.clone()]);
    assert_eq!(secrets_of(&cloud, &worker), vec!["sbxm-fake".to_owned()]);
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// `task resume --spec FILE` through `backend`, the task's process gone.
fn resume_plain(f: &Fixture, spec: &Path, backend: &FakeBackend) -> anyhow::Result<()> {
    sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec.to_path_buf()),
            rounds: None,
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

/// A start killed in its worker create, so the record holds the claim of that create.
fn start_killed_at_its_create(f: &Fixture, spec: &Path, id: &str) {
    let worker = worker_sandbox(id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        killed_at_create_of(cloud_backend(&root, id, &[CLEAN]), &worker),
        &[],
        &[&worker],
    );
    killed(|| {
        task_run(f, spec.to_path_buf(), Some(Location::Cloud), &cloud);
    });
    assert_eq!(
        task_record(f, id).creating.as_ref().map(|c| c.name.clone()),
        Some(worker)
    );
}

/// `task resume` of a start killed in its worker create removes the sandbox the claim names
/// first, the cloud may have made it, then makes the worker afresh.
#[test]
fn resume_of_a_start_killed_in_its_worker_create_removes_the_claimed_name_first() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    start_killed_at_its_create(&f, &spec, &id);
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(cloud_backend(&root, &id, &[CLEAN]), &[], &[&worker]);

    resume_plain(&f, &spec, &cloud).unwrap();

    let log = cloud.log();
    assert_eq!(log[0], format!("rm {worker}"), "{log:?}");
    assert!(
        log.iter().any(|l| l == &format!("create {worker}")),
        "{log:?}"
    );
    let task = task_record(&f, &id);
    assert!(task.creating.is_none(), "{:?}", task.creating);
    assert!(cloud.live_cloud().is_empty(), "{log:?}");
}

/// `task resume` of a start killed before it claimed its worker's name (nothing of its own can
/// be in the cloud) removes nothing before making the worker.
#[test]
fn resume_of_a_start_killed_before_its_claim_removes_nothing_first() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    start_killed_at_its_create(&f, &spec, &id);
    let mut task = task_record(&f, &id);
    task.creating = None;
    record::write(&meta(&f, &id), &task).unwrap();
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(cloud_backend(&root, &id, &[CLEAN]), &[], &[]);

    resume_plain(&f, &spec, &cloud).unwrap();

    let log = cloud.log();
    let created = log
        .iter()
        .position(|l| l == &format!("create {worker}"))
        .expect("the worker is made");
    assert!(
        !log[..created].iter().any(|l| l.starts_with("rm ")),
        "{log:?}"
    );
}

/// The one scoped secret these tests give a task, with a fake value (no production path fills
/// one in while P8 (a) holds); `sbx` names its service `postman-echo-com`.
fn fake_secret() -> sbxm::backend::ScopedSecret {
    sbxm::backend::ScopedSecret {
        host: "postman-echo.com".into(),
        env: "P2_FAKE".into(),
        value: "fake-value-never-shown".into(),
    }
}

/// `task start --spec FILE --cloud` with `secrets` through `backend`.
fn start_with_secrets(
    f: &Fixture,
    spec: &Path,
    backend: &FakeBackend,
    secrets: &[sbxm::backend::ScopedSecret],
) -> anyhow::Result<()> {
    let options = common::cloud_task::run_options(f, spec.to_path_buf(), Some(Location::Cloud));
    sbxm::commands::task_start::run_spec_with_seams(
        &f.env.config_dir(),
        &sbxm::commands::task_start::SpecOptions {
            repo_root: options.repo_root,
            spec: options.spec,
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: options.base,
            repo: options.repo,
            clone_source: options.clone_source,
            identity: options.identity,
            disk: Default::default(),
            location: Some(Location::Cloud),
        },
        backend,
        &FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
        &|_| {},
        secrets,
    )
}

/// The review of the started task `id` through `backend`, with `secrets` as the task's scoped
/// secrets (the review phase gets them from its environment).
fn review_with_secrets(
    f: &Fixture,
    id: &str,
    backend: &FakeBackend,
    secrets: &[sbxm::backend::ScopedSecret],
) -> anyhow::Result<()> {
    let mut prepared = sbxm::task::pipeline::Prepared::open(&f.env.base_dir(), id)?;
    let who = identity();
    let env = sbxm::task::pipeline::TaskEnv {
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend,
        probe: &common::task_fixture::Probe,
        host: &sbxm::task::gates::FakeHostRunner::default(),
        sleep: &|_| {},
        scoped_secrets: secrets,
        identity: Some(&who),
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).map(|_| ())
}

/// The refusal of a reviewer name the cloud already has, with scoped secrets (review round 3,
/// M-1): nothing is set, listed or removed under that name, so the secrets scoped to the
/// sandbox that was there stay; the task's own worker still goes with its secret.
#[test]
fn a_refused_reviewer_name_keeps_the_secrets_scoped_to_the_sandbox_already_there() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        cloud_backend(&root, &id, &[CLEAN]),
        &[&reviewer],
        &[&reviewer],
    );
    let secrets = [fake_secret()];
    start_with_secrets(&f, &spec, &cloud, &secrets).unwrap();
    let started = cloud.log().len();

    let message = format!(
        "{:#}",
        review_with_secrets(&f, &id, &cloud, &secrets).unwrap_err()
    );

    assert!(
        message.contains(&format!("cloud sandbox {reviewer} already exists")),
        "{message}"
    );
    assert!(touches(&cloud, &reviewer).is_empty(), "{:?}", cloud.log());
    assert_eq!(secrets_of(&cloud, &reviewer), vec!["sbxm-fake".to_owned()]);
    assert_eq!(cloud.live_cloud(), vec![reviewer.clone()]);
    let after: Vec<String> = cloud.log()[started..]
        .iter()
        .filter(|l| l.starts_with("rm ") || l.starts_with("secret-"))
        .cloned()
        .collect();
    assert_eq!(
        after,
        [
            format!("rm {worker}"),
            format!("secret-ls {worker}"),
            format!("secret-rm {worker} postman-echo-com"),
        ],
        "{:?}",
        cloud.log()
    );
}

/// A reviewer create that returned an error, with scoped secrets: the secrets it set under the
/// name are taken back at once, no sandbox of that name is removed (the guard of slice 1's
/// M-1), and the record holds nothing of it, so `task rm` leaves the name alone.
#[test]
fn a_reviewer_create_that_failed_takes_its_secrets_back_and_removes_no_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let root = f.env.tmp.path().join("cloud-fs");
    let cloud = one_cloud(
        cloud_backend(&root, &id, &[CLEAN]).with_failing_create_for(&reviewer),
        &[],
        &[],
    );
    let secrets = [fake_secret()];
    start_with_secrets(&f, &spec, &cloud, &secrets).unwrap();
    let started = cloud.log().len();

    assert!(review_with_secrets(&f, &id, &cloud, &secrets).is_err());

    let after: Vec<String> = cloud.log()[started..]
        .iter()
        .filter(|l| l.starts_with("rm ") || l.starts_with("secret-") || l.starts_with("create "))
        .cloned()
        .collect();
    assert_eq!(
        after,
        [
            format!("secret-set {reviewer} postman-echo-com"),
            format!("create {reviewer}"),
            format!("secret-ls {reviewer}"),
            format!("secret-rm {reviewer} postman-echo-com"),
            format!("rm {worker}"),
            format!("secret-ls {worker}"),
            format!("secret-rm {worker} postman-echo-com"),
        ],
        "{:?}",
        cloud.log()
    );
    let task = task_record(&f, &id);
    assert!(task.creating.is_none(), "{:?}", task.creating);
    assert!(!task.holds_sandbox(&reviewer));
    assert!(
        cloud.cloud_secrets().is_empty(),
        "{:?}",
        cloud.cloud_secrets()
    );

    let (result, out) = rm(&f, &spec, &cloud);

    result.expect(&out);
    assert!(!cloud.removes().contains(&reviewer), "{:?}", cloud.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}
