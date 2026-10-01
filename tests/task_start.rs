//! M2b slice 6, step 4: starting several picks at once (`--workers N`, decision 144): each task
//! gets its own folders, sandbox and thread, and one failing does not stop the others.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, backend, ctx, fixture, issue_text, ok, play_tasks, source,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::headless::RunStatus;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo;

fn what() -> Play {
    Play {
        commits: vec!["a.txt".into()],
        result_md: Some(b"done\n".to_vec()),
        bundle_bytes: None,
    }
}

fn playing(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what()))
}

fn github_with(numbers: &[u32]) -> FakeGitHub {
    numbers.iter().fold(FakeGitHub::default(), |gh, n| {
        gh.with_issue_text(issue_text(*n))
    })
}

fn meta(f: &Fixture, number: u32) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), &format!("issue-{number}"))
}

#[test]
fn each_pick_gets_its_own_task_sandbox_and_collected_commits() {
    let f = fixture();
    let (backend, github) = (playing(&f), github_with(&[1, 2, 3]));
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2, 3]);

    assert_eq!(
        reports.iter().map(|r| r.number).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for report in &reports {
        let worked = report.result.as_ref().unwrap();
        assert_eq!(worked.status, RunStatus::Completed, "#{}", report.number);
        assert_eq!(worked.commits, 1, "#{}", report.number);
        let task = record::read(&meta(&f, report.number).join("task.json")).unwrap();
        // The worker finished and its gates (the fixture's defaults, in the fake sandbox) passed.
        assert_eq!((task.stage, task.status), (Stage::Gating, Status::Passed));
        assert_eq!(
            task.worker.unwrap().sandbox,
            format!("sbxm-task-issue-{}-claude", report.number)
        );
        assert_eq!(
            repo::commits_ahead(
                &meta(&f, report.number).join("repo.git"),
                "main",
                &format!("issue-{}", report.number)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            fs::read_to_string(meta(&f, report.number).join("result.md")).unwrap(),
            "done\n"
        );
    }
    let mut created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    created.sort();
    assert_eq!(
        created,
        [
            "sbxm-task-issue-1-claude",
            "sbxm-task-issue-2-claude",
            "sbxm-task-issue-3-claude"
        ]
    );
}

#[test]
fn the_issue_text_comes_from_github_once_per_pick() {
    let f = fixture();
    let (backend, github) = (playing(&f), github_with(&[4, 5]));
    let source = source(&f);

    pipeline::start(&ctx(&f, &source, &backend, &github), &[4, 5]);

    let calls = github.calls();
    assert!(calls.contains(&GhCall::Issue("o/r".into(), 4)), "{calls:?}");
    assert!(calls.contains(&GhCall::Issue("o/r".into(), 5)), "{calls:?}");
    assert_eq!(calls.len(), 2, "{calls:?}");
}

#[test]
fn one_pick_failing_does_not_stop_the_others() {
    let f = fixture();
    let backend = playing(&f).with_failing_create_for("sbxm-task-issue-2-claude");
    let github = github_with(&[1, 2, 3]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2, 3]);

    assert!(reports[0].result.is_ok());
    let message = format!("{:#}", reports[1].result.as_ref().unwrap_err());
    assert!(message.contains("sbxm-task-issue-2-claude"), "{message}");
    assert!(reports[2].result.is_ok());
    assert!(!meta(&f, 2).exists(), "the failed pick left no task folder");
    assert!(meta(&f, 1).join("task.json").is_file() && meta(&f, 3).join("task.json").is_file());
}

#[test]
fn an_issue_github_cannot_show_fails_only_that_pick() {
    let f = fixture();
    let backend = playing(&f);
    let github = github_with(&[1]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 9]);

    assert!(reports[0].result.is_ok());
    let message = format!("{:#}", reports[1].result.as_ref().unwrap_err());
    assert!(message.contains("issue 9"), "{message}");
    assert!(!meta(&f, 9).exists());
}

#[test]
fn the_picks_really_run_at_the_same_time() {
    let f = fixture();
    // Both agents' commands wait at a gate until two of them are waiting: only parallel
    // execution can get past it; a sequential start would hang, so the gate has a deadline.
    let (backend, gate) = playing(&f).with_exec_gate();
    let github = github_with(&[1, 2]);
    let source = source(&f);
    let releaser = std::thread::spawn({
        let gate = gate.clone();
        move || {
            gate.wait_for_blocked(2);
            gate.open();
        }
    });

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2]);

    releaser.join().unwrap();
    assert!(reports.iter().all(|r| r.result.is_ok()));
}
