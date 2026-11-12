//! Issue 597: a task's record stops naming a process when that process's run of it ends,
//! whatever the outcome (failed, parked, ready, finished, a refusal after the record was
//! written), so `task resume` refuses only while a command really still works on the task. The
//! loop runs these same commands in its own process, which stays alive: every test here uses a
//! probe that says every process is alive, as the loop's would.

mod common;

use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, ctx, fixture_with, ok, play, play_reviews,
    source, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{task_finish, task_resume, task_review, task_status};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Process, Record, Stage, Status};

const TOML: &str = "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
    [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n";

const CLEAN: &str = "Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n";
/// A review with no `Must-fix findings:` line: the review fails.
const UNUSABLE: &str = "Looks fine.\n";

fn failed() -> ExecOutput {
    ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    }
}

fn saved(f: &Fixture, id: &str) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap()
}

/// The worker commits `a.txt` in task `id`'s clone; the reviewer writes `reviews` in turn.
fn backend_for(f: &Fixture, id: &str, reviews: &[&str]) -> FakeBackend {
    let worker = play(
        &f.env.base_dir().join("tasks").join(id),
        "main",
        id,
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        id,
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

fn resume(
    f: &Fixture,
    target: task_resume::Target,
    backend: &FakeBackend,
) -> (anyhow::Result<()>, String) {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_resume::run(
        &f.env.config_dir(),
        &task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target,
            rounds: None,
            should_fix: false,
            disk: Default::default(),
            identity: None,
            forget_ambiguous: false,
        },
        backend,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    (result, String::from_utf8(out).unwrap())
}

fn assert_not_refused_as_running(result: &anyhow::Result<()>) {
    if let Err(e) = result {
        let message = format!("{e:#}");
        assert!(!message.contains("still running"), "{message}");
    }
}

/// A spec task prepared and worked (as `task start --spec` leaves it), its spec file's path.
fn worked_spec(f: &Fixture) -> (PathBuf, String) {
    let spec = common::task_fixture::spec_file(f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(f, &id, &[]);
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_spec(&context, &spec).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    (spec, id)
}

fn review_spec(f: &Fixture, spec: &Path, backend: &FakeBackend) -> anyhow::Result<()> {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_review::run(
        &f.env.config_dir(),
        &task_review::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: task_review::Target::Spec(spec.to_owned()),
            repo: Some("o/r".into()),
            base: None,
            clone_source: None,
            reviewer_harness: None,
            reviewer_model: None,
            reviewer_time_limit: None,
            time_limit: None,
            profile: None,
            resolved_spec_id: None,
            force: false,
            disk: Default::default(),
            location: None,
            identity: None,
        },
        backend,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )
}

/// The 2026-11-10 case: a spec task whose review fails names no process afterwards, and a
/// resume run while the process that reviewed it still lives goes ahead.
#[test]
fn a_spec_task_whose_review_fails_names_no_process_and_resumes() {
    let f = fixture_with(TOML);
    let (spec, id) = worked_spec(&f);

    let result = review_spec(&f, &spec, &backend_for(&f, &id, &[UNUSABLE]));

    assert!(result.is_err());
    let task = saved(&f, &id);
    assert_eq!(
        (task.stage, task.status),
        (Stage::Reviewing, Status::Failed)
    );
    assert_eq!(task.process, None);

    let (result, out) = resume(
        &f,
        task_resume::Target::Spec(spec),
        &backend_for(&f, &id, &[CLEAN]),
    );

    assert_not_refused_as_running(&result);
    result.unwrap();
    assert!(out.contains(&format!("{id}: ready")), "{out}");
    let task = saved(&f, &id);
    assert_eq!((task.stage, task.status), (Stage::Ready, Status::Ok));
    assert_eq!(task.process, None);
}

/// Issue 41 worked once, then left `working/failed` by a run that has ended.
fn failed_issue_task(f: &Fixture) {
    let mut prepared = worked_task(f, &backend_for(f, "issue-41", &[]));
    prepared.record.status = Status::Failed;
    prepared.record.process = None;
    record::write(&prepared.meta, &prepared.record).unwrap();
}

/// An issue task the loop parks (its resume fails a second time) names no process, and the
/// next `task resume --issue` goes ahead; one that reaches ready names none either.
#[test]
fn an_issue_task_whose_resume_fails_again_names_no_process_and_resumes_to_ready() {
    let f = fixture_with(TOML);
    failed_issue_task(&f);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", failed());

    let (result, _) = resume(&f, task_resume::Target::Issue(41), &failing);

    assert_not_refused_as_running(&result);
    let task = saved(&f, "issue-41");
    assert_eq!((task.stage, task.status), (Stage::Working, Status::Failed));
    assert_eq!(task.process, None);

    let (result, out) = resume(
        &f,
        task_resume::Target::Issue(41),
        &backend_for(&f, "issue-41", &[CLEAN]),
    );

    assert_not_refused_as_running(&result);
    result.unwrap();
    assert!(out.contains("issue-41: ready"), "{out}");
    let task = saved(&f, "issue-41");
    assert_eq!((task.stage, task.status), (Stage::Ready, Status::Ok));
    assert_eq!(task.process, None);
}

/// A finished task names no process: `task finish` ran it to its end.
#[test]
fn a_finished_task_names_no_process() {
    let f = fixture_with(TOML);
    failed_issue_task(&f);
    let (result, _) = resume(
        &f,
        task_resume::Target::Issue(41),
        &backend_for(&f, "issue-41", &[CLEAN]),
    );
    result.unwrap();

    let mut out = Vec::new();
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Issue(41),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &mut out,
    )
    .unwrap();

    let task = saved(&f, "issue-41");
    assert_eq!(task.stage, Stage::Finished);
    assert_eq!(task.process, None);
}

/// A task another live process works on is still refused, and the refusal leaves that process
/// named: only a run's own process is cleared at its end.
#[test]
fn a_task_another_live_process_works_on_is_refused_and_keeps_its_process() {
    let f = fixture_with(TOML);
    failed_issue_task(&f);
    let dir = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut task = saved(&f, "issue-41");
    task.status = Status::Running;
    task.process = Some(Process::new(1, 0));
    record::write(&dir, &task).unwrap();
    let backend = backend_for(&f, "issue-41", &[CLEAN]);

    let (result, _) = resume(&f, task_resume::Target::Issue(41), &backend);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("still running in sbxm process pid 1"),
        "{message}"
    );
    assert!(backend.execs().is_empty());
    assert_eq!(saved(&f, "issue-41").process, Some(Process::new(1, 0)));
}

/// `task status` and `sbxm status` show a failed task with no process as failed.
#[test]
fn a_failed_task_with_no_process_shows_failed_in_both_status_views() {
    let f = fixture_with(TOML);
    failed_issue_task(&f);
    let base = f.env.base_dir();

    let (text, _) = task_status::render(&base, None, false, &Probe).unwrap();
    let line = text.lines().find(|l| l.starts_with("issue-41")).unwrap();
    assert!(line.contains("failed"), "{text}");
    assert!(
        !line.contains("interrupted") && !line.contains("running"),
        "{text}"
    );

    let rows = sbxm::status::tasks::rows(&base, &Probe).unwrap();
    let row = rows.iter().find(|r| r.id == "issue-41").unwrap();
    assert_eq!(row.status, "failed");
}
