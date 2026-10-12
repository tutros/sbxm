//! Decision 185 (issue #207): `--restart`'s discard is undoable. These tests exercise the
//! `finish::SetAside` mechanics directly (set aside, restore, settle) rather than through a whole
//! restart command, since the command-level behavior is already covered by
//! `tests/task_start_restart.rs`, `tests/task_run.rs` and `tests/task_run_spec.rs`.

mod common;

use std::fs;

use common::task_fixture::{Fixture, Probe, fixture, worked_task};
use sbxm::backend::FakeBackend;
use sbxm::confirm::Confirm;
use sbxm::task::pipeline::{Prepared, TaskEnv};
use sbxm::task::record::{self, ProcessProbe, Stage, Status};
use sbxm::task::{finish, resume};

/// The process a record names is gone (the `sbxm` that wrote it was killed): needed to resume a
/// task left `Working`/`Failed`, since [`Probe`] (used for the set-aside mechanics themselves)
/// always reports the recorded process as still alive.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// A `Confirm` that must never be asked: `set_aside`/`set_aside_many` with `yes: true` never
/// prompts.
struct Never;

impl Confirm for Never {
    fn is_interactive(&self) -> bool {
        panic!("not asked")
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        panic!("not asked")
    }
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

/// Issue 41, prepared and worked once, left `Working`/`Failed` as a crash would leave it: a
/// resumable state ([`resume::resume`] reruns the worker for it).
fn resumable_task(f: &Fixture, backend: &FakeBackend) -> Prepared {
    let mut prepared = worked_task(f, backend);
    prepared.record.stage = Stage::Working;
    prepared.record.status = Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared
}

fn host() -> &'static sbxm::task::gates::FakeHostRunner {
    Box::leak(Box::default())
}

fn env<'a>(f: &'a Fixture, backend: &'a FakeBackend) -> TaskEnv<'a> {
    TaskEnv {
        config_dir: Box::leak(Box::new(f.env.config_dir())),
        config: &f.config,
        backend,
        probe: &Gone,
        host: host(),
    }
}

/// Decision 185 point 2: the old task's sandbox is removed at the discard (not undone by
/// `restore`), so a restored task has none; its `task.json` still names it, and `task resume`
/// accepts the task and makes the sandbox again rather than refusing it.
#[test]
fn a_restored_task_has_no_sandbox_but_resume_makes_it_again() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let prepared = resumable_task(&f, &backend);
    let sandbox = prepared.record.worker.clone().unwrap().sandbox;
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(backend.removes(), vec![sandbox.clone()]);
    aside.restore().unwrap();

    // The restored record still names the (now gone) sandbox: `restore` only moves folders.
    let restored = saved(&f);
    assert_eq!(restored.worker.unwrap().sandbox, sandbox);
    assert!(meta(&f).join("repo.git").is_dir());

    let mut resuming = Prepared::open(&base_dir, "issue-41").unwrap();
    let fresh = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let resumed = resume::resume(&env(&f, &fresh), &mut resuming, None).unwrap();

    assert!(resumed.sandbox_remade, "resume must make the sandbox again");
    assert!(
        fresh.creates().iter().any(|c| c.name == sandbox),
        "{:?}",
        fresh.creates()
    );
}

/// Decision 185 point 3: once the new task's preparation is complete, the set-aside copy is gone
/// for good — here simulated directly by calling `settle`, since the command-level tests already
/// cover the full restart flow.
#[test]
fn settling_deletes_the_set_aside_copy_for_good() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(finish::has_leftover(&base_dir, "issue-41"));

    aside.settle().unwrap();

    assert!(!finish::has_leftover(&base_dir, "issue-41"));
    assert!(
        !base_dir.join(".sbxm").join("tasks-replaced").exists()
            || fs::read_dir(base_dir.join(".sbxm").join("tasks-replaced"))
                .unwrap()
                .next()
                .is_none(),
        "nothing left under tasks-replaced/"
    );
}

/// Decision 185 point 4: when putting the old task back fails, the error names both failures and
/// the set-aside folder, and the folder still holds the old task (nothing more is deleted).
#[test]
fn a_failed_restore_names_both_failures_and_keeps_the_old_task_set_aside() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(finish::has_leftover(&base_dir, "issue-41"));

    // Block the restore: put a plain file where the meta folder needs to go back.
    let meta_dir = meta(&f);
    fs::create_dir_all(meta_dir.parent().unwrap()).unwrap();
    fs::write(&meta_dir, b"in the way").unwrap();

    let start_failed = anyhow::anyhow!("the worker's secret is missing");
    let combined = finish::restore_after_failed_start(&aside, start_failed);
    let message = format!("{combined:#}");

    assert!(
        message.contains("the worker's secret is missing"),
        "{message}"
    );
    assert!(message.contains("putting the old task"), "{message}");
    assert!(
        message.contains(&aside.folder().display().to_string()),
        "{message}"
    );
    assert!(
        finish::has_leftover(&base_dir, "issue-41"),
        "the old task is still set aside"
    );
    // Nothing more was deleted: the blocking file is still exactly what it was.
    assert_eq!(fs::read(&meta_dir).unwrap(), b"in the way");
}

/// Decision 185 point 4: a `--restart` of a task with a leftover copy is refused before anything
/// is checked, naming the folder and the two ways out.
#[test]
fn check_no_leftover_refuses_and_names_the_folder_and_the_two_ways_out() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();
    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();

    let message = format!(
        "{:#}",
        finish::check_no_leftover(&base_dir, "issue-41").unwrap_err()
    );

    assert!(
        message.contains(&aside.folder().display().to_string()),
        "{message}"
    );
    assert!(message.contains("move it back"), "{message}");
    assert!(message.contains("delete it"), "{message}");
}
