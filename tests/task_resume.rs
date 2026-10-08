//! Issue 118 (decisions 173(c), 175; spec `sdlc/specs/task-state-machine.md` section 5.2):
//! `task resume` continues a task from its recorded stage, keeping its clone and commits, and
//! refuses while the recorded `sbxm` process is still alive.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, issue_text, ok, play_reviews,
    play_tasks, source, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend, SandboxInfo};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Prepared, TaskEnv};
use sbxm::task::record::{self, Kind, ProcessProbe, ReviewResult, Stage, Status, Stopped};
use sbxm::task::{repo, resume};

fn config() -> Fixture {
    config_with_fix_rounds(1)
}

fn config_with_fix_rounds(fix_rounds: u32) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = {fix_rounds}\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    ))
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n1. must-fix: a.txt:1 does the wrong thing.\n";

/// A structured must-fix finding, so a later round can claim to repeat it (spec §5.3).
const FINDING1: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n";

/// Validly repeats `FINDING1`'s `M-1` (same file).
const REPEAT: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt still does the wrong thing\n\n\
    **Where:** `a.txt:5`\n\
    **What happens:** it still returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n\
    **Repeat of:** M-1\n";

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

/// Issue 41 prepared (folders, clone, record, sandbox) but its worker never started, left in
/// `status` as a killed or failed preparation would leave it.
fn prepared_left_in(f: &Fixture, status: Status) -> Prepared {
    let first = backend(f, &[CLEAN]);
    let github = FakeGitHub::default();
    let source = source(f);
    let mut prepared =
        pipeline::prepare(&ctx(f, &source, &first, &github), &issue_text(41)).unwrap();
    prepared.record.status = status;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared
}

// ---- The preparation (T9, F4; decision 175) ----

#[test]
fn an_interrupted_preparation_makes_the_sandbox_again_then_runs_the_worker() {
    let f = config();
    let mut prepared = prepared_left_in(&f, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    // A cut-off `sbx create` may have left a half-made sandbox: it goes first.
    assert_eq!(backend.removes()[0], "sbxm-task-issue-41-claude");
    assert!(resumed.sandbox_remade);
    assert!(
        backend
            .creates()
            .iter()
            .any(|c| c.name == "sbxm-task-issue-41-claude")
    );
    assert_eq!(resumed.worked.unwrap().commits, 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_failed_preparation_is_retried_the_same_way() {
    let f = config();
    let mut prepared = prepared_left_in(&f, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "claude"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn a_preparation_whose_sandbox_cannot_be_made_stays_prepared_and_keeps_its_folders() {
    let f = config();
    let mut prepared = prepared_left_in(&f, Status::Running);
    let backend = backend(&f, &[CLEAN]).with_failing_create_for("sbxm-task-issue-41-claude");

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("sbxm-task-issue-41-claude"), "{message}");
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Failed)
    );
    assert!(meta(&f).join("repo.git").is_dir());
    assert!(f.env.base_dir().join("tasks").join("issue-41").is_dir());
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

// ---- The review and the gates (T3, T9) ----

#[test]
fn an_interrupted_review_runs_the_reviewer_again_without_the_gates() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Reviewing, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert!(resumed.worked.is_none());
    assert_eq!(count(&backend, "claude"), 0, "no worker");
    assert_eq!(
        count(&backend, "cargo test"),
        0,
        "the gates passed before the review"
    );
    assert_eq!(count(&backend, "codex"), 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_failed_review_runs_the_reviewer_again() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Reviewing, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "codex"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn interrupted_gates_run_again_then_the_review() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Gating, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "cargo test"), 1);
    assert_eq!(count(&backend, "codex"), 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(
        !record.gates.iter().any(|g| !g.passed),
        "an abandoned gate run is never recorded as failed"
    );
}

#[test]
fn a_worker_that_completed_but_was_never_reviewed_goes_on_to_the_review() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Completed);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "claude"), 0);
    assert_eq!(count(&backend, "cargo test"), 1);
    assert_eq!(count(&backend, "codex"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn commits_a_cut_off_collection_missed_are_collected_before_the_review() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Completed);
    // The worker committed once more, and its process was killed before the bundle was fetched.
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    std::fs::write(workspace.join("b.txt"), "late\n").unwrap();
    common::git(&workspace, &["add", "-A"]);
    common::git(&workspace, &["commit", "-q", "-m", "late"]);
    let before = commits(&f);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(commits(&f), before + 1);
}

#[test]
fn a_fix_round_that_ended_but_was_never_counted_is_counted() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Fixing, Status::Completed);
    assert_eq!(prepared.record.round, 0);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    let record = saved(&f);
    assert_eq!(record.round, 1);
    assert_eq!(record.stage, Stage::Ready);
}

#[test]
fn a_failed_review_whose_worker_sandbox_is_gone_gets_it_back_for_the_gates_and_fix_rounds() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Gating, Status::GatesFailed);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert!(resumed.sandbox_remade);
    assert!(
        backend
            .creates()
            .iter()
            .any(|c| c.name == "sbxm-task-issue-41-claude")
    );
    assert_eq!(saved(&f).stage, Stage::Ready);
}

// ---- A review that completed, its result never acted on (T4) ----

/// Issue 41 left at `reviewing/completed` (its process gone before it moved on), with `review`
/// saved as `review.md` and its result recorded as `result`.
fn reviewed_task(f: &Fixture, review: &str, result: Option<ReviewResult>) -> Prepared {
    let mut prepared = task_left_in(f, Stage::Reviewing, Status::Completed);
    std::fs::write(meta(f).join("review.md"), review).unwrap();
    std::fs::write(meta(f).join("review-1.md"), review).unwrap();
    // A review with findings narrows the next one to the commits after the one it saw.
    if result.as_ref().is_some_and(|r| r.must_fix > 0) {
        prepared.record.last_reviewed_commit =
            Some(repo::branch_tip(&meta(f).join("repo.git"), "issue-41").unwrap());
    }
    prepared.record.review = result;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared
}

#[test]
fn a_review_records_its_result_when_it_completes() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    resume_free_review(&f, &backend, &mut prepared);

    assert_eq!(
        saved(&f).review,
        Some(ReviewResult {
            round: 1,
            must_fix: 0,
            full: true,
            repeat: false,
        })
    );
}

fn resume_free_review(f: &Fixture, backend: &FakeBackend, prepared: &mut Prepared) {
    pipeline::review_issue(&env(f, backend, &Gone), prepared).unwrap();
}

#[test]
fn a_clean_full_review_is_replayed_to_ready_without_running_the_reviewer() {
    let f = config();
    let result = ReviewResult {
        round: 1,
        must_fix: 0,
        full: true,
        repeat: false,
    };
    let mut prepared = reviewed_task(&f, CLEAN, Some(result));
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "codex"), 0);
    assert_eq!(count(&backend, "cargo test"), 0);
    assert!(resumed.review.unwrap().rounds.is_empty(), "nothing new ran");
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_review_with_findings_is_replayed_into_the_fix_round_it_would_have_started() {
    let f = config();
    let result = ReviewResult {
        round: 1,
        must_fix: 1,
        full: true,
        repeat: false,
    };
    let mut prepared = reviewed_task(&f, ONE, Some(result));
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "fix-prompt.md"), 1, "one fix round");
    // After the fix: the gates, a narrow review, then the confirming full one.
    assert_eq!(count(&backend, "codex"), 2);
    let rounds: Vec<u32> = resumed
        .review
        .unwrap()
        .rounds
        .iter()
        .map(|r| r.round)
        .collect();
    assert_eq!(rounds, [2, 3], "the replayed review was round 1");
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.round, 1);
}

#[test]
fn a_record_without_the_result_runs_the_reviewer_again() {
    // A record written before the result was recorded: `review.md` is not read back instead.
    let f = config();
    let mut prepared = reviewed_task(&f, ONE, None);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "fix-prompt.md"), 0);
    assert_eq!(count(&backend, "codex"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

// ---- The fix round (T5, T9) ----

/// Issue 41 whose first fix round failed: the review found one must-fix finding, and the
/// worker's fix run errored (`fixing/failed`, the round not counted).
fn failed_fix_for_a_review(f: &Fixture) -> Prepared {
    let first = backend(f, &[ONE]);
    let mut prepared = worked_task(f, &first);
    let failing = backend(f, &[ONE]).with_failing_exec_matching("fix-prompt.md");
    pipeline::review_issue(&env(f, &failing, &Gone), &mut prepared).unwrap_err();
    let record = saved(f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Fixing, Status::Failed)
    );
    prepared
}

#[test]
fn a_failed_fix_round_runs_again_from_its_review_and_the_task_goes_on_to_ready() {
    let f = config();
    let mut prepared = failed_fix_for_a_review(&f);
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(
        count(&backend, "fix-prompt.md"),
        1,
        "the fix round ran again"
    );
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    assert_eq!(
        std::fs::read_to_string(workspace.join(".sbxm-task").join("review.md")).unwrap(),
        std::fs::read_to_string(meta(&f).join("review-1.md")).unwrap(),
        "the worker sees the review it is fixing"
    );
    let report = resumed.review.unwrap();
    assert!(report.fix_ran);
    // After the fix: a narrow review, then the confirming full one.
    let rounds: Vec<(u32, bool)> = report.rounds.iter().map(|r| (r.round, r.full)).collect();
    assert_eq!(rounds, [(2, false), (3, true)]);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.round, 1, "the round is counted once");
}

#[test]
fn an_interrupted_fix_round_runs_again() {
    let f = config();
    let mut prepared = failed_fix_for_a_review(&f);
    prepared.record.status = Status::Running;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "fix-prompt.md"), 1);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn a_failed_fix_round_for_a_gate_runs_again_with_the_gate_s_output() {
    let f = config();
    let first = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &first);
    let failing = backend(&f, &[CLEAN])
        .with_exec_output_matching(
            "cargo test",
            ExecOutput {
                stdout: "test a ... FAILED\n".into(),
                stderr: String::new(),
                exit_code: Some(1),
            },
        )
        .with_failing_exec_matching("fix-prompt.md");
    pipeline::review_issue(&env(&f, &failing, &Gone), &mut prepared).unwrap_err();
    assert_eq!(saved(&f).stage, Stage::Fixing);
    let backend = backend(&f, &[CLEAN]);

    resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    let input = f
        .env
        .base_dir()
        .join("tasks")
        .join("issue-41")
        .join(".sbxm-task")
        .join("gate-failure.md");
    let text = std::fs::read_to_string(input).unwrap();
    assert!(
        text.contains("cargo test") && text.contains("FAILED"),
        "{text}"
    );
    assert_eq!(count(&backend, "fix-prompt.md"), 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.round), (Stage::Ready, 1));
}

// ---- A task that stopped ready (`--rounds N`, decisions 173(c), 177(b)(q)) ----

/// Issue 41 reviewed through `reviews` with the config's budget, ending `ready` with `stopped`.
fn stopped_task(f: &Fixture, reviews: &[&str]) -> Prepared {
    let first = backend(f, reviews);
    let mut prepared = worked_task(f, &first);
    pipeline::review_issue(&env(f, &first, &Gone), &mut prepared).unwrap();
    let record = saved(f);
    assert_eq!(record.stage, Stage::Ready);
    assert!(record.stopped.is_some());
    prepared
}

#[test]
fn rounds_n_raises_the_budget_and_starts_a_fix_round_from_the_review() {
    let f = config();
    // Round 1 finds one, the one fix round runs, round 2 still finds one: rounds-exhausted.
    let mut prepared = stopped_task(&f, &[ONE, ONE]);
    assert_eq!(prepared.record.stopped, Some(Stopped::RoundsExhausted));
    let review = std::fs::read_to_string(meta(&f).join("review.md")).unwrap();
    let backend = backend(&f, &[CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, Some(2)).unwrap();

    assert_eq!(count(&backend, "fix-prompt.md"), 1);
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    assert_eq!(
        std::fs::read_to_string(workspace.join(".sbxm-task").join("review.md")).unwrap(),
        review,
        "the fix round works from the review the task stopped on"
    );
    let rounds: Vec<(u32, bool)> = resumed
        .review
        .unwrap()
        .rounds
        .iter()
        .map(|r| (r.round, r.full))
        .collect();
    assert_eq!(rounds, [(3, false), (4, true)]);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!((record.round, record.fix_rounds), (2, 3));
    assert_eq!(record.stopped, None, "a successful resume clears it");
}

#[test]
fn plain_resume_of_a_task_that_used_all_its_rounds_is_refused_and_names_rounds() {
    let f = config();
    let mut prepared = stopped_task(&f, &[ONE, ONE]);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap_err()
    );

    assert!(message.contains("--rounds"), "{message}");
    assert!(message.contains("rounds-exhausted"), "{message}");
    assert!(backend.execs().is_empty());
}

#[test]
fn rounds_zero_is_refused() {
    let f = config();
    let mut prepared = stopped_task(&f, &[ONE, ONE]);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, Some(0)).unwrap_err()
    );

    assert!(message.contains("1 or more"), "{message}");
    assert_eq!(saved(&f).fix_rounds, 1);
}

#[test]
fn rounds_on_a_task_that_did_not_stop_ready_is_refused() {
    let f = config();
    let mut prepared = task_left_in(&f, Stage::Working, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    let message = format!(
        "{:#}",
        resume::resume(&env(&f, &backend, &Gone), &mut prepared, Some(1)).unwrap_err()
    );

    assert!(message.contains("without --rounds"), "{message}");
    assert!(backend.execs().is_empty());
}

#[test]
fn a_ready_task_that_did_not_stop_is_refused_and_names_finish() {
    let f = config();
    let first = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &first);
    pipeline::review_issue(&env(&f, &first, &Gone), &mut prepared).unwrap();
    let backend = backend(&f, &[CLEAN]);

    for rounds in [None, Some(1)] {
        let message = format!(
            "{:#}",
            resume::resume(&env(&f, &backend, &Gone), &mut prepared, rounds).unwrap_err()
        );
        assert!(message.contains("task finish --issue 41"), "{message}");
    }
    assert!(backend.execs().is_empty());
}

#[test]
fn a_repeat_stop_with_rounds_left_resumes_without_rounds_and_its_first_review_ignores_repeats() {
    let f = config_with_fix_rounds(3);
    // Round 1 finds M-1, the fix round runs, round 2 repeats it: stopped with 2 rounds left.
    let mut prepared = stopped_task(&f, &[FINDING1, REPEAT]);
    assert_eq!(prepared.record.stopped, Some(Stopped::RepeatFinding));
    assert_eq!(prepared.record.round, 1);
    // After the resumed fix round, round 3 repeats M-1 again: ignored once, so another fix round
    // runs instead of an immediate stop; then round 4 (narrow) and 5 (full) are clean.
    let backend = backend(&f, &[REPEAT, CLEAN, CLEAN]);

    let resumed = resume::resume(&env(&f, &backend, &Gone), &mut prepared, None).unwrap();

    assert_eq!(count(&backend, "fix-prompt.md"), 2);
    let rounds: Vec<u32> = resumed
        .review
        .unwrap()
        .rounds
        .iter()
        .map(|r| r.round)
        .collect();
    assert_eq!(rounds, [3, 4, 5]);
    let record = saved(&f);
    assert_eq!((record.stage, record.stopped), (Stage::Ready, None));
    assert_eq!((record.round, record.fix_rounds), (3, 3));
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
