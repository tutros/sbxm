//! Issue 118 (decisions 173(c), 175; spec `sdlc/specs/task-state-machine.md` section 5.2):
//! `task resume` continues a task from its recorded stage, keeping its clone and commits, and
//! refuses while the recorded `sbxm` process is still alive.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks, worked_task,
};
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{Prepared, TaskEnv};
use sbxm::task::record::{self, Kind, ProcessProbe, Stage, Status};
use sbxm::task::{repo, resume};

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

/// The process a record names is gone (the `sbxm` that wrote it was killed).
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// The process a record names is still running (the fixture records every process as started at 0).
struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(0)
    }
}

/// The worker commits `a.txt` each time it runs; the n-th review is `reviews[n]`.
fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn env<'a>(f: &'a Fixture, backend: &'a FakeBackend, probe: &'a dyn ProcessProbe) -> TaskEnv<'a> {
    let host: &'static FakeHostRunner = Box::leak(Box::default());
    TaskEnv {
        config_dir: Box::leak(Box::new(f.env.config_dir())),
        config: &f.config,
        backend,
        probe,
        host,
    }
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

/// Issue 41 with its worker run once (one commit), then left in `stage`/`status` as a crash or a
/// failure would leave it.
fn task_left_in(f: &Fixture, stage: Stage, status: Status) -> Prepared {
    let first = backend(f, &[CLEAN]);
    let mut prepared = worked_task(f, &first);
    prepared.record.stage = stage;
    prepared.record.status = status;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared
}

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

fn commits(f: &Fixture) -> u32 {
    repo::commits_ahead(&meta(f).join("repo.git"), "main", "issue-41").unwrap()
}

// ---- The worker (T6, T9) ----

#[test]
fn a_failed_worker_runs_again_in_its_clone_and_the_task_goes_on_to_ready() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let before = commits(&f);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    let worked = resumed.worked.expect("the worker ran again");
    assert!(worked.commits > before, "the first run's commit is kept");
    assert_eq!(commits(&f), before + 1);
    assert_eq!(count(&backend, "claude"), 1, "the worker ran once more");
    assert_eq!(
        count(&backend, "cargo test"),
        1,
        "gates ran before the review"
    );
    assert_eq!(count(&backend, "codex"), 1, "then the reviewer");
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(resumed.review.is_some());
}

#[test]
fn an_interrupted_worker_runs_again() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "claude"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn the_worker_s_sandbox_is_made_again_when_it_is_gone() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert!(resumed.sandbox_remade);
    let creates: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert!(
        creates.contains(&"sbxm-task-issue-41-claude".to_owned()),
        "{creates:?}"
    );
}

#[test]
fn the_worker_s_sandbox_is_kept_when_it_still_exists() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let backend = backend(&f, &[CLEAN]).and_sandboxes(vec![SandboxInfo {
        name: "sbxm-task-issue-41-claude".into(),
        agent: "claude".into(),
        status: "running".into(),
    }]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert!(!resumed.sandbox_remade);
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name == "sbxm-task-issue-41-claude")
    );
}

#[test]
fn a_worker_that_fails_again_stays_failed_and_is_not_reviewed() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let backend =
        FakeBackend::with_secrets(&["anthropic", "openai"]).with_failing_exec_matching("claude");

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None);

    assert!(resumed.is_err() || resumed.as_ref().unwrap().review.is_none());
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Working, Status::Failed)
    );
    assert_eq!(count(&backend, "codex"), 0, "no reviewer");
}

// ---- Refusals ----

#[test]
fn a_running_worker_whose_process_is_alive_is_refused_and_nothing_runs() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Alive), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("still running"), "{message}");
    assert!(message.contains("pid"), "{message}");
    assert!(backend.execs().is_empty() && backend.creates().is_empty());
    assert_eq!(saved(&f).status, Status::Running);
}

#[test]
fn a_failed_task_whose_process_is_still_alive_is_refused() {
    // A task between two stages (a `task review` that just saw its worker fail, say) is not
    // `running`, but its process is: resuming it would race that process.
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Alive), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("still running"), "{message}");
    assert!(backend.execs().is_empty());
}

#[test]
fn a_pr_task_is_refused() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Reviewing, Status::Failed);
    prepared.record.kind = Kind::Pr;
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("PR task"), "{message}");
    assert!(message.contains("task review --pr"), "{message}");
    assert!(backend.execs().is_empty());
}

#[test]
fn a_finished_task_is_refused() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Finished, Status::Ok);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("finished"), "{message}");
    assert!(backend.execs().is_empty());
}
