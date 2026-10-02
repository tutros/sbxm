//! M2b slice 6, step 3: the worker run and collecting its commits and `result.md`
//! (spec §5.1 steps 3 and 4), with the fake backend playing the agent.

mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, backend, ctx, fixture, issue_text, ok, play, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::headless::RunStatus;
use sbxm::run::orchestrate::in_sandbox_path;
use sbxm::task::pipeline::{self, Prepared, Worked};
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo;

fn workspace(f: &Fixture) -> std::path::PathBuf {
    f.env.base_dir().join("tasks").join("issue-41")
}

/// A backend whose headless command finishes and whose sandbox plays `what`.
fn playing(f: &Fixture, what: Play) -> FakeBackend {
    playing_with(f, what, ok(CLAUDE_DONE))
}

/// Like [`playing`], with the headless command ending as `headless` says.
fn playing_with(f: &Fixture, what: Play, headless: ExecOutput) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", headless)
        .with_exec_hook(play(&workspace(f), "main", "issue-41", what))
}

fn start(f: &Fixture, backend: &FakeBackend) -> (Prepared, anyhow::Result<Worked>) {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx(f, &source, backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &issue_text(41)).unwrap();
    let worked = pipeline::run_worker(&ctx, &mut prepared);
    (prepared, worked)
}

fn saved(f: &Fixture) -> record::Record {
    record::read(
        &f.env
            .base_dir()
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .join("task.json"),
    )
    .unwrap()
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    f.env
        .base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-41")
}

fn play_commits(files: &[&str]) -> Play {
    Play {
        commits: files.iter().map(|s| (*s).to_owned()).collect(),
        result_md: Some(b"- [x] criterion: `cargo test` ok\n".to_vec()),
        bundle_bytes: None,
    }
}

#[test]
fn a_finished_worker_leaves_its_commits_result_and_transcript_on_the_host() {
    let f = fixture();
    let backend = playing(&f, play_commits(&["a.txt", "b.txt"]));

    let (prepared, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert_eq!(worked.status, RunStatus::Completed);
    assert!(worked.notes.is_empty(), "{:?}", worked.notes);
    assert_eq!(worked.commits, 2);
    let repo_git = meta(&f).join("repo.git");
    assert_eq!(
        repo::commits_ahead(&repo_git, "main", "issue-41").unwrap(),
        2
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("result.md")).unwrap(),
        "- [x] criterion: `cargo test` ok\n"
    );
    assert!(
        fs::read_to_string(meta(&f).join("transcripts").join("worker.jsonl"))
            .unwrap()
            .contains("PONG")
    );

    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Working, Status::Completed)
    );
    assert_eq!(
        prepared.record, record,
        "the in-memory record matches what was saved"
    );
    let run = record.worker.unwrap().run.unwrap();
    assert_eq!(run.status, "completed");
    assert!(run.usage["output_tokens"].is_number(), "{}", run.usage);
}

#[test]
fn the_agent_is_run_under_the_timeout_on_a_prompt_file_and_the_bundle_command_is_fixed() {
    let f = fixture();
    let backend = playing(&f, play_commits(&["a.txt"]));

    let (prepared, worked) = start(&f, &backend);
    worked.unwrap();

    let execs = backend.execs();
    assert_eq!(execs.len(), 3, "headless, bundle, status: {execs:?}");
    let (sandbox, headless) = &execs[0];
    assert_eq!(sandbox, "sbxm-task-issue-41-claude");
    assert_eq!(&headless.argv[..2], ["timeout", "-v"]);
    assert!(
        headless
            .argv
            .iter()
            .any(|a| a.contains(".sbxm-task/prompt.md")),
        "{:?}",
        headless.argv
    );
    assert_eq!(
        headless.workdir.as_deref(),
        Some(in_sandbox_path(&prepared.workspace).as_path())
    );

    let ws = in_sandbox_path(&prepared.workspace)
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        execs[1].1.argv,
        [
            "git",
            "-C",
            &ws,
            "bundle",
            "create",
            ".sbxm-task/branch.bundle",
            "issue-41",
            "^origin/main"
        ]
    );
    assert_eq!(execs[2].1.argv, ["git", "-C", &ws, "status", "--porcelain"]);
}

#[test]
fn the_working_stage_is_on_disk_before_the_agent_starts() {
    let f = fixture();
    let seen = Arc::new(Mutex::new(None));
    let seen_in_hook = Arc::clone(&seen);
    let record_path = meta(&f).join("task.json");
    let player = play(&workspace(&f), "main", "issue-41", play_commits(&["a.txt"]));
    let backend = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if spec.argv.iter().any(|a| a == "claude") {
                let record = record::read(&record_path).unwrap();
                *seen_in_hook.lock().unwrap() = Some((record.stage, record.status));
            }
            player(sandbox, spec);
        });

    let (_, worked) = start(&f, &backend);
    worked.unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        Some((Stage::Working, Status::Running))
    );
}

#[test]
fn a_timed_out_worker_is_recorded_and_its_partial_commits_are_still_collected() {
    let f = fixture();
    let backend = playing_with(
        &f,
        play_commits(&["a.txt"]),
        ExecOutput {
            stdout: String::new(),
            stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
            exit_code: Some(124),
        },
    );

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert_eq!(worked.status, RunStatus::TimedOut);
    assert_eq!(saved(&f).status, Status::TimedOut);
    assert_eq!(worked.commits, 1);
}

#[test]
fn a_failed_worker_is_recorded_with_its_reason() {
    let f = fixture();
    let backend = playing_with(
        &f,
        play_commits(&["a.txt"]),
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );

    let (_, worked) = start(&f, &backend);

    assert!(matches!(worked.unwrap().status, RunStatus::Failed(why) if why.contains("boom")));
    let record = saved(&f);
    assert_eq!(record.status, Status::Failed);
    assert!(record.worker.unwrap().run.unwrap().status.contains("boom"));
}

#[test]
fn a_run_that_cannot_be_attempted_is_recorded_failed_and_returned_as_an_error() {
    let f = fixture();
    let backend = backend().with_failing_exec_matching("claude");

    let (_, worked) = start(&f, &backend);

    assert!(worked.is_err());
    assert_eq!(saved(&f).status, Status::Failed);
}

#[test]
fn a_worker_with_no_commits_is_noted_not_an_error() {
    let f = fixture();
    let backend = playing(&f, Play::default()).with_exec_output_matching(
        "bundle",
        ExecOutput {
            stdout: String::new(),
            stderr: "fatal: Refusing to create empty bundle.\n".into(),
            exit_code: Some(128),
        },
    );

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert_eq!(worked.commits, 0);
    assert!(
        worked.notes.iter().any(|n| n.contains("no commits")),
        "{:?}",
        worked.notes
    );
    assert_eq!(
        saved(&f).notes,
        worked.notes,
        "notes are saved with the record"
    );
}

#[test]
fn uncommitted_changes_in_the_clone_are_reported_as_a_note() {
    let f = fixture();
    let backend = playing(&f, play_commits(&["a.txt"]))
        .with_exec_output_matching("status", ok(" M src/a.rs\n?? notes.txt\n"));

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert!(
        worked.notes.iter().any(|n| n.contains("2 uncommitted")),
        "{:?}",
        worked.notes
    );
}

#[test]
fn a_missing_result_md_is_noted() {
    let f = fixture();
    let mut what = play_commits(&["a.txt"]);
    what.result_md = None;
    let backend = playing(&f, what);

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert!(
        worked.notes.iter().any(|n| n.contains("no result.md")),
        "{:?}",
        worked.notes
    );
    assert!(!meta(&f).join("result.md").exists());
}

#[test]
fn an_oversized_result_md_is_not_copied() {
    let f = fixture();
    let mut what = play_commits(&["a.txt"]);
    what.result_md = Some(vec![b'x'; 2 * 1024 * 1024]);
    let backend = playing(&f, what);

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert!(
        worked
            .notes
            .iter()
            .any(|n| n.contains("result.md") && n.contains("larger")),
        "{:?}",
        worked.notes
    );
    assert!(!meta(&f).join("result.md").exists());
}

#[test]
fn a_bundle_that_fails_verification_marks_the_worker_failed_and_changes_nothing() {
    let f = fixture();
    let mut what = play_commits(&["a.txt"]);
    what.bundle_bytes = Some(b"not a bundle".to_vec());
    let backend = playing(&f, what);

    let (_, worked) = start(&f, &backend);
    let worked = worked.unwrap();

    assert!(
        matches!(worked.status, RunStatus::Failed(_)),
        "{:?}",
        worked.status
    );
    let record = saved(&f);
    assert_eq!(record.status, Status::Failed);
    assert!(
        record.notes.iter().any(|n| n.contains("could not collect")),
        "{:?}",
        record.notes
    );
    let repo_git = meta(&f).join("repo.git");
    assert_eq!(
        repo::commits_ahead(&repo_git, "main", "issue-41").unwrap(),
        0
    );
    // The clone the agent worked in is still there for inspection.
    assert!(git(&workspace(&f), &["log", "--oneline"]).lines().count() >= 2);
}
