//! Issue 612 (decision 321(2)): every task command writes `outcome.json` in its task's folder as
//! it ends, by hand too: `{"end": "<kind>", "provider": ..., "detail": ...}`.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, issue_text, ok, open_issue,
    play_reviews, play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_run::{Options, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::outcome::{self, Kind, Outcome, Read};
use sbxm::task::record;
use sbxm::task::repo::Identity;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn backend(f: &Fixture) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &[], "")])
}

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issue: 41,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    }
}

fn task_run(f: &Fixture, backend: &FakeBackend) -> anyhow::Result<()> {
    run(
        &f.env.config_dir(),
        &options(f),
        None,
        backend,
        &github(),
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

fn dir(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

#[test]
fn task_run_by_hand_writes_ready() {
    let f = config();

    task_run(&f, &backend(&f)).unwrap();

    assert_eq!(
        outcome::read(&dir(&f)),
        Read::Outcome(Outcome::new(Kind::Ready, None, ""))
    );
    assert!(!dir(&f).join("outcome.json.tmp").exists());
}

#[test]
fn a_failed_worker_is_failed_with_the_error() {
    let f = config();
    let b = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );

    let e = task_run(&f, &b).unwrap_err();

    let Read::Outcome(o) = outcome::read(&dir(&f)) else {
        panic!("{:?}", outcome::read(&dir(&f)))
    };
    assert_eq!(o.end, Kind::Failed);
    assert_eq!(o.detail, format!("{e:#}"));
}

#[test]
fn a_refusal_before_the_task_folder_writes_nothing() {
    let f = config();
    // The reviewer's secret is missing: refused before anything is created.
    let b = FakeBackend::with_secrets(&["anthropic"]);

    task_run(&f, &b).unwrap_err();

    assert!(!dir(&f).exists(), "no task folder made for the outcome");
}

#[test]
fn an_earlier_runs_outcome_is_not_left_as_this_ones() {
    let f = config();
    task_run(&f, &backend(&f)).unwrap();
    // An outcome from some earlier run says it ended on a usage limit.
    outcome::write(
        &dir(&f),
        &Outcome::new(Kind::UsageLimit, Some("anthropic".into()), "old"),
    )
    .unwrap();

    // `task run` again on the existing task: refused, the record kept.
    task_run(&f, &backend(&f)).unwrap_err();

    let Read::Outcome(o) = outcome::read(&dir(&f)) else {
        panic!()
    };
    assert_eq!(o.end, Kind::Failed, "{o:?}");
    assert_ne!(o.detail, "old");
}

#[test]
fn a_command_that_fails_to_name_its_task_leaves_an_earlier_outcome_alone_only_for_other_tasks() {
    let f = config();
    task_run(&f, &backend(&f)).unwrap();
    let other = record::task_dir(&f.env.base_dir(), "issue-42");
    fs::create_dir_all(&other).unwrap();
    let kept = Outcome::new(Kind::Ready, None, "");
    outcome::write(&other, &kept).unwrap();

    task_run(&f, &backend(&f)).unwrap_err();

    assert_eq!(outcome::read(&other), Read::Outcome(kept));
}
