//! M2b slice 8, step 4: the whole review of an issue task (spec §5.2): gates first, a reviewer,
//! at most one fix round by the worker, gates again, one more review, then `ready`.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play_reviews, play_tasks,
    source, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, Prepared, ReviewReport};
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo;

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n1. must-fix: a.txt:1 does the wrong thing.\n";

/// The worker (and a fix round) commit a file each time; the n-th review is `reviews[n]`.
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

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

fn review(
    f: &Fixture,
    backend: &FakeBackend,
    prepared: &mut Prepared,
) -> anyhow::Result<ReviewReport> {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::review_issue(&ctx(f, &source, backend, &github), prepared)
}

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

#[test]
fn a_clean_review_runs_the_gates_first_and_ends_ready_without_a_fix_round() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report
            .rounds
            .iter()
            .map(|r| (r.round, r.must_fix))
            .collect::<Vec<_>>(),
        [(1, 0)]
    );
    assert_eq!(report.must_fix_left, 0);
    assert!(!report.fix_ran && report.gates_failed.is_none());
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(!record.fix_round);
    // The gates ran (before the review), the reviewer ran once, no fix prompt was used.
    assert_eq!(count(&backend, "cargo test"), 1);
    assert_eq!(count(&backend, "codex"), 1);
    assert_eq!(count(&backend, "fix-prompt.md"), 0);
    assert!(
        fs::read_to_string(meta(&f).join("review.md"))
            .unwrap()
            .contains("Nothing found.")
    );
}

#[test]
fn gates_that_already_passed_are_not_run_again_before_the_review() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    // `task start` already gated it: gating/passed.
    let github = FakeGitHub::default();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);
    pipeline::run_gates(
        &context.gate_env(),
        &mut prepared,
        "after-worker",
        pipeline::Tiers::ALL,
    )
    .unwrap();
    let gate_runs = count(&backend, "cargo test");

    review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(count(&backend, "cargo test"), gate_runs);
}

#[test]
fn must_fix_findings_get_one_fix_round_then_gates_and_a_second_review() {
    let f = config();
    let backend = backend(&f, &[ONE, CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report
            .rounds
            .iter()
            .map(|r| (r.round, r.must_fix))
            .collect::<Vec<_>>(),
        [(1, 1), (2, 0)]
    );
    assert!(report.fix_ran);
    assert_eq!(report.must_fix_left, 0);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(record.fix_round);
    // Gates before round 1 and again after the fix; two reviews; one fix run in the worker's sandbox.
    assert_eq!(count(&backend, "cargo test"), 2);
    assert_eq!(count(&backend, "codex"), 2);
    let fix_execs: Vec<_> = backend
        .execs()
        .into_iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains("fix-prompt.md")))
        .collect();
    assert_eq!(fix_execs.len(), 1);
    assert_eq!(
        fix_execs[0].0, "sbxm-task-issue-41-claude",
        "the worker's own sandbox"
    );

    // The worker saw the review and a fix prompt that names it; its new commit was collected.
    let workspace = f
        .env
        .base_dir()
        .join("tasks")
        .join("issue-41")
        .join(".sbxm-task");
    assert!(
        fs::read_to_string(workspace.join("review.md"))
            .unwrap()
            .contains("a.txt:1 does the wrong thing")
    );
    let fix_prompt = fs::read_to_string(workspace.join("fix-prompt.md")).unwrap();
    assert!(
        fix_prompt.contains("#41") && fix_prompt.contains(".sbxm-task/review.md"),
        "{fix_prompt}"
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("fix-prompt.md")).unwrap(),
        fix_prompt
    );
    assert_eq!(
        repo::commits_ahead(&meta(&f).join("repo.git"), "main", "issue-41").unwrap(),
        2
    );
    assert!(fs::read_to_string(meta(&f).join("transcripts").join("fix.jsonl")).is_ok());
    // Both reviews are kept; review.md is the latest.
    assert!(
        fs::read_to_string(meta(&f).join("review-1.md"))
            .unwrap()
            .contains("must-fix")
    );
    assert!(
        fs::read_to_string(meta(&f).join("review-2.md"))
            .unwrap()
            .contains("Nothing found.")
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("review.md")).unwrap(),
        fs::read_to_string(meta(&f).join("review-2.md")).unwrap()
    );
    // The reviewer's sandbox is gone after each round.
    let removed = backend.removes();
    assert_eq!(
        removed
            .iter()
            .filter(|n| n.as_str() == "sbxm-task-issue-41-review-codex")
            .count(),
        2
    );
}

#[test]
fn the_fix_round_happens_at_most_once_and_findings_left_are_reported_not_an_error() {
    let f = config();
    let backend = backend(&f, &[ONE, ONE]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(report.must_fix_left, 1);
    assert_eq!(report.rounds.len(), 2);
    assert_eq!(count(&backend, "codex"), 2, "no third review");
    assert_eq!(count(&backend, "fix-prompt.md"), 1, "no second fix round");
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Ready, Status::Ok),
        "the round is used"
    );
}

#[test]
fn failing_gates_before_the_review_stop_it_before_any_reviewer_exists() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(101),
        },
    );

    let report = review(&f, &failing, &mut prepared).unwrap();

    assert_eq!(report.gates_failed.as_ref().unwrap().command, "cargo test");
    assert!(report.rounds.is_empty());
    assert_eq!(count(&failing, "codex"), 0);
    assert!(failing.creates().is_empty(), "no reviewer sandbox");
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::GatesFailed)
    );
}

#[test]
fn failing_gates_after_the_fix_round_stop_before_the_second_review() {
    let f = config();
    // Execs that no scripted rule matches are answered from this queue, in order: the worker's
    // bundle and status, the gates before the review (pass), the fix round's bundle and status,
    // then the gates after the fix round (fail).
    let pass = || ok("");
    let red = ExecOutput {
        stdout: String::new(),
        stderr: "red\n".into(),
        exit_code: Some(1),
    };
    let backend = backend(&f, &[ONE, CLEAN]).with_exec_outputs(vec![
        pass(),
        pass(),
        pass(),
        pass(),
        pass(),
        red,
    ]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert_eq!(report.gates_failed.as_ref().unwrap().phase, "after-fix");
    assert_eq!(report.rounds.len(), 1, "no second review");
    assert_eq!(count(&backend, "codex"), 1);
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::GatesFailed)
    );
    assert!(record.fix_round);
}

#[test]
fn a_failed_fix_round_is_an_error_recorded_on_the_task() {
    let f = config();
    let reviews = backend(&f, &[ONE]);
    let mut prepared = worked_task(&f, &reviews);
    // The worker's headless run now fails (exit 1) when the fix prompt is used.
    let failing_fix = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching(
            "fix-prompt.md",
            ExecOutput {
                stdout: String::new(),
                stderr: "boom".into(),
                exit_code: Some(1),
            },
        )
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook({
            let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![ONE.into()]);
            move |sandbox, spec| reviewer(sandbox, spec)
        });

    let message = format!("{:#}", review(&f, &failing_fix, &mut prepared).unwrap_err());

    assert!(
        message.contains("fix round") && message.contains("boom"),
        "{message}"
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Fixing, Status::Failed)
    );
    assert_eq!(count(&failing_fix, "codex"), 1, "no second review");
}

#[test]
fn a_reviewer_failure_is_an_error_and_the_task_says_so() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    let broken =
        FakeBackend::with_secrets(&["anthropic", "openai"]).with_failing_exec_matching("codex");

    assert!(review(&f, &broken, &mut prepared).is_err());

    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Failed)
    );
    assert!(
        broken
            .removes()
            .contains(&"sbxm-task-issue-41-review-codex".to_owned())
    );
}

#[test]
fn after_a_failed_review_it_can_be_run_again() {
    let f = config();
    let good = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &good);
    let broken =
        FakeBackend::with_secrets(&["anthropic", "openai"]).with_failing_exec_matching("codex");
    assert!(review(&f, &broken, &mut prepared).is_err());

    let report = review(&f, &good, &mut prepared).unwrap();

    assert_eq!(report.must_fix_left, 0);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn the_reviewers_secret_is_checked_before_the_gates_or_anything_else() {
    let f = config();
    let ready = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &ready);
    let before = saved(&f);
    let no_openai = FakeBackend::with_secrets(&["anthropic"]);

    let message = format!("{:#}", review(&f, &no_openai, &mut prepared).unwrap_err());

    assert!(message.contains("openai"), "{message}");
    assert!(no_openai.execs().is_empty() && no_openai.creates().is_empty());
    assert_eq!(saved(&f), before, "nothing changed");
}

#[test]
fn a_task_that_cannot_be_reviewed_now_is_refused_with_why() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    // Still running.
    prepared.record.status = Status::Running;
    let message = format!("{:#}", review(&f, &backend, &mut prepared).unwrap_err());
    assert!(message.contains("worker still running"), "{message}");

    // A failed worker.
    prepared.record.status = Status::Failed;
    let message = format!("{:#}", review(&f, &backend, &mut prepared).unwrap_err());
    assert!(message.contains("nothing to review"), "{message}");

    // Already reviewed (ready).
    prepared.record.stage = Stage::Ready;
    prepared.record.status = Status::Ok;
    let message = format!("{:#}", review(&f, &backend, &mut prepared).unwrap_err());
    assert!(
        message.contains("already") && message.contains("ready"),
        "{message}"
    );
}
