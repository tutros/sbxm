//! Milestone 4 slice 6 (`sdlc/milestone-4.md`, work-order item 6; decisions 253, 259, 262(e),
//! 265): `task rm` of a cloud task removes, in the cloud, every sandbox its `task.json` still
//! lists as not removed (retried; one already gone counts as removed), then the secrets scoped
//! to each, then its folders. The record decides where, whatever the config says. Through
//! `FakeBackend`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use common::cloud_task::{
    CLEAN, cloud_backend, reviewer_sandbox, spec, task_record, task_run, worker_sandbox,
};
use common::task_fixture::{Fixture, Probe, fixture_with};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_rm::{Options, Target, run_with_sleep};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record;

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// `sbx --cloud rm` of a sandbox the cloud doesn't know (`sbx-cloud-rm-not-found.txt`).
fn not_found(name: &str) -> String {
    format!(
        "sbx --cloud rm {name} -f failed: error: sandbox '{name}' not found\n  try: sbx --cloud ls"
    )
}

const UNAVAILABLE: &str = "error: sandbox temporarily unavailable, try again";

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

/// A cloud spec task run to `ready` (both of its sandboxes removed at the stop), with the config
/// saying `local` from then on: the record alone says `cloud`.
fn ready_cloud_task() -> (Fixture, PathBuf, String) {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let ran = task_run(
        &f,
        spec.clone(),
        Some(Location::Cloud),
        &cloud_backend(&root, &id, &[CLEAN]),
    );
    ran.result.unwrap();
    (f, spec, id)
}

/// Marks the lifetime entry of `name` as never removed, as a removal that failed at the task's
/// stop leaves it.
fn reopen(f: &Fixture, id: &str, name: &str) {
    let mut task = task_record(f, id);
    for entry in task.sandboxes.iter_mut().filter(|e| e.name == name) {
        entry.removed_at = None;
    }
    record::write(&record::task_dir(&f.env.base_dir(), id), &task).unwrap();
}

/// The repo's config says `cloud` from now on.
fn to_cloud_config(f: &Fixture) {
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();
}

fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

/// `task rm --spec FILE --yes --force` through `backend`, recording each wait between retries.
fn rm(f: &Fixture, spec: &Path, backend: &FakeBackend) -> (anyhow::Result<()>, String, Vec<u64>) {
    let waits = Mutex::new(Vec::new());
    let mut out = Vec::new();
    let result = run_with_sleep(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec.to_path_buf()),
            yes: true,
            force: true,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
        },
        backend,
        &FakeGitHub::default(),
        &Probe,
        &Never,
        &mut out,
        &|wait: Duration| waits.lock().unwrap().push(wait.as_secs()),
    );
    (
        result,
        String::from_utf8(out).unwrap(),
        waits.into_inner().unwrap(),
    )
}

/// Boundary case: two sandbox entries, one removed and one not. One `rm` call, in the cloud, then
/// the secrets scoped to that sandbox; the folder goes.
#[test]
fn rm_of_a_cloud_task_removes_only_the_sandbox_still_listed_then_its_secrets() {
    let (f, spec, id) = ready_cloud_task();
    let worker = worker_sandbox(&id);
    reopen(&f, &id, &worker);
    let backend = FakeBackend::default()
        .with_cloud_secret(&worker, "sbxm-fake")
        .with_cloud_secret(&reviewer_sandbox(&id), "sbxm-other");

    let (result, out, waits) = rm(&f, &spec, &backend);

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
    assert!(waits.is_empty(), "{waits:?}");
    assert!(!meta(&f, &id).exists(), "{out}");
    assert!(
        out.contains(&format!("removed cloud sandbox {worker}")),
        "{out}"
    );
}

/// Boundary case: a removal that still fails after its 3 retries. The folder is kept, nothing is
/// stamped, and the message names the command that removes the sandbox by hand.
#[test]
fn rm_of_a_cloud_task_whose_sandbox_cannot_be_removed_keeps_the_task() {
    let (f, spec, id) = ready_cloud_task();
    let worker = worker_sandbox(&id);
    reopen(&f, &id, &worker);
    let before = std::fs::read(meta(&f, &id).join("task.json")).unwrap();
    let backend = FakeBackend::default().with_remove_errors(&[UNAVAILABLE; 4]);

    let (result, out, waits) = rm(&f, &spec, &backend);

    let err = format!("{:#}", result.unwrap_err());
    assert_eq!(backend.removes(), vec![worker.clone(); 4]);
    assert_eq!(waits, vec![5, 10, 20]);
    assert!(
        out.contains(&format!("sbx --cloud rm {worker} -f")),
        "{out}\n{err}"
    );
    assert!(meta(&f, &id).join("task.json").is_file(), "{out}");
    assert_eq!(
        std::fs::read(meta(&f, &id).join("task.json")).unwrap(),
        before
    );
    assert!(err.contains("stayed"), "{err}");
}

/// Boundary case: a sandbox already gone counts as removed, and the folder goes.
#[test]
fn rm_of_a_cloud_task_whose_sandbox_is_already_gone_removes_the_folder() {
    let (f, spec, id) = ready_cloud_task();
    let worker = worker_sandbox(&id);
    reopen(&f, &id, &worker);
    let backend = FakeBackend::default().with_remove_errors(&[&not_found(&worker)]);

    let (result, out, waits) = rm(&f, &spec, &backend);

    result.unwrap();
    assert_eq!(backend.removes(), vec![worker.clone()]);
    assert!(waits.is_empty());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// A cloud task with every sandbox already removed: no `rm` at all, and the folder goes.
#[test]
fn rm_of_a_cloud_task_with_every_sandbox_removed_calls_no_rm() {
    let (f, spec, id) = ready_cloud_task();
    let backend = FakeBackend::default();

    let (result, out, _) = rm(&f, &spec, &backend);

    result.unwrap();
    assert!(backend.removes().is_empty(), "{:?}", backend.log());
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The reverse: a task recorded `local` while the config says `cloud` is removed here, with the
/// local `sbx rm` of its sandboxes, never in the cloud.
#[test]
fn rm_of_a_local_task_under_a_cloud_config_removes_it_here() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = common::task_fixture::ready_spec_task(&f);
    to_cloud_config(&f);
    let backend = FakeBackend::default();

    let (result, out, _) = rm(&f, &spec, &backend);

    result.unwrap();
    assert!(!backend.locations().contains(&Location::Cloud), "{out}");
    assert!(
        backend
            .removes()
            .contains(&format!("sbxm-task-{id}-claude")),
        "{:?}",
        backend.log()
    );
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// The cloud task finished through the `local` sink, then merged into `main` the way
/// `tests/task_merged_remove.rs`'s `merged_spec_task` merges one; returns the checkout that
/// `task rm --merged` runs from.
fn merge_cloud_task(f: &Fixture, spec: &Path, id: &str) -> PathBuf {
    sbxm::commands::task_finish::run(
        &f.env.config_dir(),
        &sbxm::commands::task_finish::Options {
            target: sbxm::commands::task_finish::Target::Spec(spec.to_path_buf()),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    let repo_git = meta(f, id).join("repo.git");
    let branch = format!("{id}:{id}");
    let (checkout, scratch) = (
        f.env.tmp.path().join("checkout"),
        f.env.tmp.path().join("merge-scratch"),
    );
    for dir in [&checkout, &scratch] {
        common::git(
            f.env.tmp.path(),
            &[
                "clone",
                "-q",
                f.origin.to_str().unwrap(),
                dir.to_str().unwrap(),
            ],
        );
        common::git(dir, &["fetch", "-q", repo_git.to_str().unwrap(), &branch]);
    }
    common::git(&scratch, &["merge", "--ff-only", "-q", id]);
    common::git(&scratch, &["push", "-q", "origin", "main"]);
    checkout
}

/// Boundary case: `task rm --merged` of a merged cloud task archives its `task.json` with the
/// `sandboxes` list first (decision 265), then removes the sandbox still listed, in the cloud,
/// and the folder.
#[test]
fn rm_merged_of_a_cloud_task_archives_its_lifetimes_then_removes_it_in_the_cloud() {
    let (f, spec, id) = ready_cloud_task();
    let checkout = merge_cloud_task(&f, &spec, &id);
    let worker = worker_sandbox(&id);
    reopen(&f, &id, &worker);
    // What `main` writes for a command (an in-process run has none of its own).
    let log_line = format!("cloud sandbox {worker} created (worker, TTL 3h)\n");
    std::fs::write(meta(&f, &id).join("run.log"), &log_line).unwrap();
    let backend = FakeBackend::default();
    let mut out = Vec::new();

    run_with_sleep(
        &f.env.config_dir(),
        &Options {
            target: Target::Merged,
            yes: true,
            force: false,
            repo_root: checkout,
            dry_run: false,
        },
        &backend,
        &FakeGitHub::default(),
        &Probe,
        &Never,
        &mut out,
        &|_| {},
    )
    .unwrap();

    let out = String::from_utf8(out).unwrap();
    let archived = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("task-archive")
        .join(&id);
    let task = record::read(&archived.join("task.json")).unwrap();
    let names: Vec<&str> = task.sandboxes.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec![worker.as_str(), reviewer_sandbox(&id).as_str()],
        "{out}"
    );
    assert_eq!(task.location, Location::Cloud);
    assert_eq!(
        std::fs::read_to_string(archived.join("run.log")).unwrap(),
        log_line
    );
    assert_eq!(backend.removes(), vec![worker.clone()], "{out}");
    assert_eq!(backend.locations(), vec![Location::Cloud]);
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// `task rm --merged` whose cloud removal still fails after its retries leaves the task as it is
/// (its `task.json` byte for byte), so a second run reuses its own identical archive and finishes
/// the job.
#[test]
fn rm_merged_whose_cloud_removal_fails_leaves_the_task_for_a_second_run() {
    let (f, spec, id) = ready_cloud_task();
    let checkout = merge_cloud_task(&f, &spec, &id);
    let worker = worker_sandbox(&id);
    reopen(&f, &id, &worker);
    let before = std::fs::read(meta(&f, &id).join("task.json")).unwrap();
    let merged = |backend: &FakeBackend| {
        let mut out = Vec::new();
        let result = run_with_sleep(
            &f.env.config_dir(),
            &Options {
                target: Target::Merged,
                yes: true,
                force: false,
                repo_root: checkout.clone(),
                dry_run: false,
            },
            backend,
            &FakeGitHub::default(),
            &Probe,
            &Never,
            &mut out,
            &|_| {},
        );
        (result, String::from_utf8(out).unwrap())
    };

    let (result, out) = merged(&FakeBackend::default().with_remove_errors(&[UNAVAILABLE; 4]));

    assert!(result.is_err(), "{out}");
    assert!(
        out.contains(&format!("sbx --cloud rm {worker} -f")),
        "{out}"
    );
    assert_eq!(
        std::fs::read(meta(&f, &id).join("task.json")).unwrap(),
        before
    );

    let (result, out) = merged(&FakeBackend::default());

    result.unwrap();
    assert!(!meta(&f, &id).exists(), "{out}");
}

/// A task whose `task.json` can't be read may have been a cloud task: the note names where to
/// look in the cloud too, since its sandboxes can't be named.
#[test]
fn rm_of_a_task_with_an_unreadable_record_names_the_cloud_list_too() {
    let (f, spec, id) = ready_cloud_task();
    std::fs::write(meta(&f, &id).join("task.json"), "not json").unwrap();
    let confirm = Asked::default();
    let mut out = Vec::new();

    let _ = run_with_sleep(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec),
            yes: false,
            force: true,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
        },
        &FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &confirm,
        &mut out,
        &|_| {},
    );

    let prompt = confirm.0.lock().unwrap().join("\n");
    assert!(prompt.contains("`sbx --cloud ls`"), "{prompt}");
}

/// Says no, keeping each prompt.
#[derive(Default)]
struct Asked(Mutex<Vec<String>>);

impl Confirm for Asked {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.0.lock().unwrap().push(prompt.to_owned());
        Ok(false)
    }
}
