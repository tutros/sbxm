//! Milestone 4 slice 6 (`sdlc/milestone-4.md`, work-order item 6, the Commands table's `task
//! resume` row; P5 (d); decisions 253, 261, 263): `task resume` of a task recorded `cloud` runs
//! in the cloud whatever the config says: the cloud preflight again first, a gone worker sandbox
//! made again from the recorded profile and a bundle of the branch (a new `sandboxes` entry), the
//! task continued from its recorded stage, and the worker sandbox removed when it stops. Through
//! `FakeBackend`.

mod common;

use std::path::PathBuf;

use common::cloud_task::{
    CLEAN, HOME, ONE, cloud_backend, cloud_backend_listing, cloud_backend_worker, identity,
    reviewer_sandbox, spec, task_record, task_run, worker_sandbox,
};
use common::task_fixture::{Fixture, fixture_with};
use sbxm::backend::{ExecOutput, FakeBackend, Location};
use sbxm::commands::task_resume::{Options, Target, run};
use sbxm::task::record::{self, ProcessProbe, SandboxRole, Stage, Status};

/// The process that ran the task is gone, as it is for every resume.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// What the worker's headless command gives back when it fails.
fn failed() -> ExecOutput {
    ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    }
}

/// A cloud spec task whose worker failed: it stopped `working/failed` and its worker sandbox was
/// removed at that stop. The config says `local`.
fn failed_cloud_task() -> (Fixture, PathBuf, String) {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend_worker(&root, &[&id], &[CLEAN], true, failed());
    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &backend);
    assert!(ran.result.is_err(), "{}", ran.out);
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.status), (Stage::Working, Status::Failed));
    assert!(task.sandboxes.iter().all(|e| e.removed_at.is_some()));
    (f, spec, id)
}

struct Ran {
    result: anyhow::Result<()>,
    out: String,
}

fn resume(f: &Fixture, spec: PathBuf, backend: &FakeBackend) -> Ran {
    resume_rounds(f, spec, None, backend)
}

/// `task resume --spec FILE`, with `--rounds` when given.
fn resume_rounds(f: &Fixture, spec: PathBuf, rounds: Option<u32>, backend: &FakeBackend) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: Target::Spec(spec),
            rounds,
            disk: Default::default(),
            identity: Some(identity()),
        },
        backend,
        &Gone,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: format!(
            "{}{}",
            String::from_utf8(out).unwrap(),
            String::from_utf8(warn).unwrap()
        ),
    }
}

/// A gone cloud worker sandbox is made again in the cloud (a new `worker` entry, the recorded
/// clone path), the worker runs again there, then the gates and a review, and the task ends
/// `ready` with every sandbox removed; the config's `local` changes nothing.
#[test]
fn resume_of_a_cloud_task_makes_its_gone_worker_sandbox_again_and_runs_to_ready() {
    let (f, spec, id) = failed_cloud_task();
    let worker = worker_sandbox(&id);
    let gone = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN], false);

    let ran = resume(&f, spec, &gone);

    ran.result.expect(&ran.out);
    assert!(
        gone.locations().iter().all(|l| *l == Location::Cloud),
        "{:?}",
        gone.locations()
    );
    let creates: Vec<String> = gone.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(creates, vec![worker.clone(), reviewer_sandbox(&id)]);
    for create in gone.creates() {
        assert_eq!(create.workspace, None);
        assert!(create.ttl.is_some());
    }
    let clone = format!("{HOME}/work/{id}");
    let log = gone.log();
    for file in ["source.md", "prompt.md"] {
        assert!(
            log.contains(&format!("cp-in {worker} {clone}/.sbxm-task/{file}")),
            "{log:?}"
        );
    }
    assert_eq!(log.last(), Some(&format!("rm {worker}")), "{log:?}");
    let task = task_record(&f, &id);
    assert_eq!(task.stage, Stage::Ready, "{}", ran.out);
    assert_eq!(task.worker.as_ref().unwrap().workspace, clone);
    let roles: Vec<SandboxRole> = task.sandboxes.iter().map(|e| e.role).collect();
    assert_eq!(
        roles,
        vec![
            SandboxRole::Worker,
            SandboxRole::Worker,
            SandboxRole::Reviewer
        ]
    );
    assert!(
        task.sandboxes.iter().all(|e| e.removed_at.is_some()),
        "{:?}",
        task.sandboxes
    );
    assert!(
        ran.out.contains(&format!("cloud sandbox {worker} removed")),
        "{}",
        ran.out
    );
}

/// Boundary case: a cloud task whose worker sandbox is gone and whose preflight now fails (an
/// `sbx` under the cloud minimum) is refused before any write or changing call.
#[test]
fn resume_of_a_cloud_task_whose_preflight_now_fails_changes_nothing() {
    let (f, spec, id) = failed_cloud_task();
    let before = std::fs::read(record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    let gone = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN], false)
        .with_version("0.45.0");

    let ran = resume(&f, spec, &gone);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("0.46.0"), "{err}");
    assert!(gone.log().is_empty(), "{:?}", gone.log());
    assert_eq!(
        std::fs::read(record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap(),
        before
    );
}

/// A worker that fails again stops the task, and its remade sandbox is removed at that stop.
#[test]
fn resume_of_a_cloud_task_whose_worker_fails_again_removes_its_sandbox() {
    let (f, spec, id) = failed_cloud_task();
    let worker = worker_sandbox(&id);
    let gone = cloud_backend_worker(
        &f.env.tmp.path().join("cloud-fs"),
        &[],
        &[CLEAN],
        false,
        failed(),
    );

    let ran = resume(&f, spec, &gone);

    assert!(ran.result.is_err(), "{}", ran.out);
    assert_eq!(
        gone.log().last(),
        Some(&format!("rm {worker}")),
        "{:?}",
        gone.log()
    );
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.status), (Stage::Working, Status::Failed));
    assert!(
        task.sandboxes.iter().all(|e| e.removed_at.is_some()),
        "{:?}",
        task.sandboxes
    );
}

/// The reverse: a task recorded `local` is resumed here though the config now says `cloud` (a
/// completed review replayed: no sandbox is made).
#[test]
fn resume_of_a_local_task_under_a_cloud_config_runs_here() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = common::task_fixture::ready_spec_task(&f);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    task.stage = Stage::Reviewing;
    task.status = Status::Completed;
    record::write(&meta, &task).unwrap();
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let ran = resume(&f, spec, &backend);

    ran.result.expect(&ran.out);
    assert!(
        !backend.locations().contains(&Location::Cloud),
        "{:?}",
        backend.locations()
    );
    assert_eq!(task_record(&f, &id).stage, Stage::Ready);
}

/// A cloud task that stopped `ready` with its fix rounds used up: `task resume --rounds 1` makes
/// the worker sandbox again for the fix round (a `fix` entry), reviews in a new reviewer sandbox,
/// and removes both.
#[test]
fn resume_rounds_of_a_cloud_task_stopped_ready_runs_a_fix_round_in_a_remade_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    let task = task_record(&f, &id);
    assert_eq!(task.stage, Stage::Ready);
    assert!(task.stopped.is_some(), "{task:?}");
    let entries = task.sandboxes.len();
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));
    let gone = cloud_backend_listing(&root, &id, &[CLEAN], false);

    let ran = resume_rounds(&f, spec, Some(1), &gone);

    ran.result.expect(&ran.out);
    // The fix round's sandbox, then a reviewer's per round (a clean narrow review is followed by
    // the confirmatory full one, issue 117).
    let creates: Vec<String> = gone.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(creates[0], worker);
    assert!(creates[1..].iter().all(|c| *c == reviewer), "{creates:?}");
    let clone = format!("{HOME}/work/{id}");
    assert!(
        gone.log()
            .contains(&format!("cp-in {worker} {clone}/.sbxm-task/fix-prompt.md")),
        "{:?}",
        gone.log()
    );
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.stopped), (Stage::Ready, None));
    let roles: Vec<SandboxRole> = task.sandboxes[entries..].iter().map(|e| e.role).collect();
    assert_eq!(roles[0], SandboxRole::Fix, "{roles:?}");
    assert!(
        roles.len() > 1 && roles[1..].iter().all(|r| *r == SandboxRole::Reviewer),
        "{roles:?}"
    );
    assert!(
        task.sandboxes.iter().all(|e| e.removed_at.is_some()),
        "{:?}",
        task.sandboxes
    );
}

/// A cloud fix round that failed runs again in a remade worker sandbox (a `fix` entry), its
/// prompt and review copied in from the task's folder, and the task goes on to `ready`.
#[test]
fn resume_of_a_cloud_task_whose_fix_round_failed_runs_it_again_in_a_remade_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    // As a fix round that failed leaves it, with its prompt kept in the task's folder.
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    task.stage = Stage::Fixing;
    task.status = Status::Failed;
    task.stopped = None;
    task.fix_rounds += 1;
    record::write(&meta, &task).unwrap();
    std::fs::write(meta.join("fix-prompt.md"), "Fix it.\n").unwrap();
    let entries = task.sandboxes.len();
    let worker = worker_sandbox(&id);
    let gone = cloud_backend_listing(&root, &id, &[CLEAN], false);

    let ran = resume(&f, spec, &gone);

    ran.result.expect(&ran.out);
    let clone = format!("{HOME}/work/{id}");
    let log = gone.log();
    for file in ["review.md", "fix-prompt.md"] {
        assert!(
            log.contains(&format!("cp-in {worker} {clone}/.sbxm-task/{file}")),
            "{log:?}"
        );
    }
    let task = task_record(&f, &id);
    assert_eq!(task.stage, Stage::Ready, "{}", ran.out);
    let roles: Vec<SandboxRole> = task.sandboxes[entries..].iter().map(|e| e.role).collect();
    assert_eq!(roles[0], SandboxRole::Fix, "{roles:?}");
    assert!(
        roles.len() > 1 && roles[1..].iter().all(|r| *r == SandboxRole::Reviewer),
        "{roles:?}"
    );
    assert!(
        task.sandboxes.iter().all(|e| e.removed_at.is_some()),
        "{:?}",
        task.sandboxes
    );
}

// PR 322 review M-2: a rebuilt worker sandbox's TTL comes from the task's own recorded budget
// (`fix_rounds` as it started, plus any `--rounds N`), never from what `sbxm-task.toml` says
// now; an added count whose TTL doesn't add up is refused before any write or changing call.

/// The config with one fix round and one sandbox gate of a minute.
const ONE_ROUND_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[worker]\nfix_rounds = 1\n\n[gates]\nsandbox = [\"cargo test\"]\ntimeout = \"60s\"\n";

/// The repo's `sbxm-task.toml` is now `toml`.
fn set_config(f: &Fixture, toml: &str) {
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        toml,
    )
    .unwrap();
}

fn load_config(f: &Fixture) -> sbxm::task::config::TaskConfig {
    sbxm::task::config::TaskConfig::load(&f.env.tmp.path().join("target-repo")).unwrap()
}

/// A cloud spec task under `toml`, run to `ready` with its one fix round used
/// (`rounds-exhausted`); its worker sandbox went at that stop.
fn exhausted_cloud_task(toml: &str) -> (Fixture, PathBuf, String) {
    let f = fixture_with(toml);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[ONE]),
    );
    ran.result.expect(&ran.out);
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.fix_rounds), (Stage::Ready, 1));
    assert!(task.stopped.is_some(), "{task:?}");
    (f, spec, id)
}

/// The TTL the first create of `backend` (the rebuilt worker's) was given.
fn first_ttl(backend: &FakeBackend, worker: &str) -> Option<std::time::Duration> {
    let create = backend.creates().into_iter().next().unwrap();
    assert_eq!(create.name, worker);
    create.ttl
}

/// Started with `fix_rounds = 1` and resumed from `ready` with `--rounds 2`: the rebuilt worker
/// lives for the 3 rounds the task now has.
#[test]
fn resume_rounds_gives_the_rebuilt_worker_the_ttl_of_the_task_s_new_budget() {
    let (f, spec, id) = exhausted_cloud_task(ONE_ROUND_TOML);
    let gone = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN], false);

    let ran = resume_rounds(&f, spec, Some(2), &gone);

    ran.result.expect(&ran.out);
    assert_eq!(task_record(&f, &id).fix_rounds, 3);
    let config = load_config(&f);
    assert_eq!(
        first_ttl(&gone, &worker_sandbox(&id)),
        Some(sbxm::task::lifetime::worker_ttl(&config, 3).unwrap())
    );
}

/// `[worker] fix_rounds` raised after the task started changes nothing of its rebuilt worker's
/// TTL: the task's budget is the one it recorded.
#[test]
fn a_fix_rounds_changed_after_the_start_leaves_the_rebuilt_worker_s_ttl_alone() {
    let f = fixture_with(ONE_ROUND_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend_worker(&root, &[&id], &[CLEAN], true, failed());
    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &backend);
    assert!(ran.result.is_err(), "{}", ran.out);
    assert_eq!(task_record(&f, &id).fix_rounds, 1);
    set_config(
        &f,
        &ONE_ROUND_TOML.replace("fix_rounds = 1", "fix_rounds = 5"),
    );
    let gone = cloud_backend_listing(&root, &id, &[CLEAN], false);

    let ran = resume(&f, spec, &gone);

    ran.result.expect(&ran.out);
    let config = load_config(&f);
    assert_eq!(config.fix_rounds, 5);
    assert_eq!(
        first_ttl(&gone, &worker_sandbox(&id)),
        Some(sbxm::task::lifetime::worker_ttl(&config, 1).unwrap())
    );
}

/// A reviewer time limit chosen so that a worker TTL of 2 fix rounds is the largest one that
/// adds up: the worker 60 s, one gate of 60 s, and the TTL `1920 + 2 x round` seconds.
fn at_limit_toml() -> String {
    let limit = u64::MAX / 60 * 60;
    let round = (limit - 1920) / 2 / 60 * 60;
    let reviewer = round - 120;
    format!(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[worker]\nfix_rounds = 1\n\
         time_limit = \"60s\"\n\n[reviewer]\ntime_limit = \"{reviewer}s\"\n\n[gates]\n\
         sandbox = [\"cargo test\"]\ntimeout = \"60s\"\n"
    )
}

/// Boundary, at the limit: `--rounds 1` makes the budget 2, the largest whose worker TTL adds
/// up, and the rebuilt worker gets that TTL.
#[test]
fn resume_rounds_at_the_ttl_limit_creates_the_rebuilt_worker_with_it() {
    let toml = at_limit_toml();
    let (f, spec, id) = exhausted_cloud_task(&toml);
    let config = load_config(&f);
    let ttl = sbxm::task::lifetime::worker_ttl(&config, 2).unwrap();
    assert!(sbxm::task::lifetime::worker_ttl(&config, 3).is_err());
    let gone = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN], false);

    let ran = resume_rounds(&f, spec, Some(1), &gone);

    ran.result.expect(&ran.out);
    assert_eq!(first_ttl(&gone, &worker_sandbox(&id)), Some(ttl));
}

/// Boundary, one over: `--rounds 2` makes the budget 3, whose worker TTL doesn't add up: refused
/// before any write or changing cloud call.
#[test]
fn resume_rounds_one_over_the_ttl_limit_is_refused_before_any_change() {
    let toml = at_limit_toml();
    let (f, spec, id) = exhausted_cloud_task(&toml);
    let path = record::task_dir(&f.env.base_dir(), &id).join("task.json");
    let before = std::fs::read(&path).unwrap();
    let gone = cloud_backend_listing(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN], false);

    let ran = resume_rounds(&f, spec, Some(2), &gone);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(
        err.contains("fix_rounds") && err.contains("--rounds"),
        "{err}"
    );
    assert!(gone.log().is_empty(), "{:?}", gone.log());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
