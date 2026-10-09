//! M2b slice 8, step 4, and issue 117 (fix_rounds and the multi-round loop): the whole review of
//! an issue task (spec §5.2): gates first, a reviewer, then as many fix rounds as the budget
//! allows, gates again after each, and a review scoped to the commits since the last one, until
//! one finds nothing after a full scope (or the budget runs out), then `ready`.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, ctx_with_host, fixture_with, ok, play_reviews,
    play_tasks, source, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Prepared, ReviewReport, Tiers};
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo;

/// `fix_rounds` set to 1: the tests that only care about a single fix round stay as simple as
/// before this issue's change.
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
const ONE: &str =
    "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 does the wrong thing.\n";

/// A structured finding (unlike `ONE`, which `review::must_fix_count` reads but
/// `review::repeats_a_must_fix_finding` can't parse), so round 2's `Repeat of:` line has an
/// earlier finding to validate against (issue 119).
const FINDING1: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n";

/// Claims to repeat `FINDING1`'s `M-1`, in the same file at a different line: a valid repeat.
const REPEAT_SAME_FILE: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt still does the wrong thing\n\n\
    **Where:** `a.txt:5`\n\
    **What happens:** it still returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n\
    **Repeat of:** M-1\n";

/// Claims to repeat `FINDING1`'s `M-1`, but in a different file: not a valid repeat.
const REPEAT_WRONG_FILE: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - b.txt does something else wrong\n\n\
    **Where:** `b.txt:5`\n\
    **What happens:** it does something else wrong\n\
    **Why it matters:** decision 1\n\
    **Fix:** fix it\n\
    **Repeat of:** M-1\n";

/// Claims to repeat an id `FINDING1`'s round never had: not a valid repeat.
const REPEAT_UNKNOWN_ID: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt still does the wrong thing\n\n\
    **Where:** `a.txt:5`\n\
    **What happens:** it still returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n\
    **Repeat of:** M-9\n";

/// Two structured must-fix findings in different files: `REPEAT_SAME_FILE` repeats only `M-1`.
const FINDINGS_TWO: &str = "Must-fix findings: 2\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n\n\
    ### M-2 - b.txt is wrong too\n\n\
    **Where:** `b.txt:3`\n\
    **What happens:** it is wrong\n\
    **Why it matters:** decision 2\n\
    **Fix:** make it right\n";

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
    pipeline::review_issue(&ctx(f, &source, backend, &github).env(), prepared)
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
    assert_eq!(record.round, 0);
    assert!(record.stopped.is_none());
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
        &context.env(),
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
fn must_fix_findings_get_a_fix_round_then_a_narrow_review_then_one_full_review_before_ready() {
    let f = config();
    // Round 1 (full) finds one; the fix round runs; round 2 (narrow, scoped to the commits since
    // round 1) finds nothing, so round 3 (full again) runs to confirm before the task is ready.
    let backend = backend(&f, &[ONE, CLEAN, CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report
            .rounds
            .iter()
            .map(|r| (r.round, r.must_fix, r.full))
            .collect::<Vec<_>>(),
        [(1, 1, true), (2, 0, false), (3, 0, true)]
    );
    assert!(report.fix_ran);
    assert_eq!(report.must_fix_left, 0);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.round, 1);
    assert!(record.stopped.is_none());
    // Gates before round 1 and again after the fix (not between rounds 2 and 3); three reviews;
    // one fix run in the worker's sandbox.
    assert_eq!(count(&backend, "cargo test"), 2);
    assert_eq!(count(&backend, "codex"), 3);
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
    // Every round is kept; review.md is the latest.
    assert!(
        fs::read_to_string(meta(&f).join("review-1.md"))
            .unwrap()
            .contains("### M-1 - a.txt:1")
    );
    assert!(
        fs::read_to_string(meta(&f).join("review-2.md"))
            .unwrap()
            .contains("Nothing found.")
    );
    assert!(
        fs::read_to_string(meta(&f).join("review-3.md"))
            .unwrap()
            .contains("Nothing found.")
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("review.md")).unwrap(),
        fs::read_to_string(meta(&f).join("review-3.md")).unwrap()
    );
    // The reviewer's sandbox is gone after each round.
    let removed = backend.removes();
    assert_eq!(
        removed
            .iter()
            .filter(|n| n.as_str() == "sbxm-task-issue-41-review-codex")
            .count(),
        3
    );
}

#[test]
fn a_narrow_review_is_scoped_to_the_commits_since_the_last_one() {
    let f = config();
    let backend = backend(&f, &[ONE, CLEAN, CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    let full = fs::read_to_string(meta(&f).join("reviewer-prompt-1.md")).unwrap();
    assert!(
        full.contains("git log origin/main..HEAD") && full.contains("git diff origin/main...HEAD"),
        "{full}"
    );
    let narrow = fs::read_to_string(meta(&f).join("reviewer-prompt-2.md")).unwrap();
    assert!(
        !narrow.contains("origin/main"),
        "round 2 must not be scoped from the base branch: {narrow}"
    );
    let confirmatory = fs::read_to_string(meta(&f).join("reviewer-prompt-3.md")).unwrap();
    assert!(
        confirmatory.contains("git log origin/main..HEAD"),
        "the confirmatory round is full again: {confirmatory}"
    );
}

#[test]
fn as_many_fix_rounds_as_the_budget_allows_run_before_the_task_stops() {
    let f = config_with_fix_rounds(3);
    // Every round after the first is narrow and still finds the same thing, so no confirmatory
    // full round is reached before the budget runs out.
    let backend = backend(&f, &[ONE, ONE, ONE, ONE]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report.rounds.iter().map(|r| r.must_fix).collect::<Vec<_>>(),
        [1, 1, 1, 1]
    );
    assert_eq!(report.must_fix_left, 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.round, 3, "every round in the budget ran");
    assert_eq!(record.stopped, Some(record::Stopped::RoundsExhausted));
    let fix_execs: Vec<_> = backend
        .execs()
        .into_iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains("fix-prompt.md")))
        .collect();
    assert_eq!(fix_execs.len(), 3);
}

#[test]
fn a_freshly_run_review_with_findings_at_the_last_round_number_is_refused_before_any_fix_round() {
    // Issue 169, part 2: unlike `task_resume`'s
    // `a_review_with_findings_at_the_last_round_number_is_refused_before_any_fix_round`, which
    // enters through a recorded, already-completed review and returns from `check_replay`
    // without ever reaching `review_rounds`'s own guard, this drives a review that actually runs
    // at round `u32::MAX`, so the guard right before `run_fix_round` is the one that fires. Part
    // 1 is what makes this fast: before it, building `earlier` at this round would try about 4.3
    // billion file reads first.
    let f = config_with_fix_rounds(u32::MAX);
    let backend = backend(&f, &[FINDING1]);
    let mut prepared = worked_task(&f, &backend);
    prepared.record.round = u32::MAX - 1;

    let err = review(&f, &backend, &mut prepared).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("review round"), "{message}");
    assert!(!message.contains('\n'), "{message}");
    assert!(!message.contains("move the review"), "{message}");
    assert_eq!(count(&backend, "codex"), 1, "the review itself ran");
    assert_eq!(
        count(&backend, "cargo test"),
        1,
        "only the before-review gates ran"
    );
    assert_eq!(count(&backend, "fix-prompt.md"), 0, "no fix round ran");
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
    assert_eq!(record.round, 1);
    assert_eq!(record.stopped, Some(record::Stopped::RoundsExhausted));
}

#[test]
fn a_validated_repeat_stops_the_task_before_the_round_budget_is_used() {
    let f = config_with_fix_rounds(3);
    let backend = backend(&f, &[FINDING1, REPEAT_SAME_FILE]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report.rounds.iter().map(|r| r.must_fix).collect::<Vec<_>>(),
        [1, 1]
    );
    assert_eq!(report.must_fix_left, 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(
        record.round, 1,
        "only the fix round before the repeat was found ran, though 3 were budgeted"
    );
    assert_eq!(record.stopped, Some(record::Stopped::RepeatFinding));
}

#[test]
fn a_repeat_claim_naming_a_different_file_does_not_stop_the_task() {
    let f = config_with_fix_rounds(1);
    let backend = backend(&f, &[FINDING1, REPEAT_WRONG_FILE]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(report.must_fix_left, 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    // The wrong claim is treated as a new finding: the round budget (1) is what stops it, not
    // the no-progress rule.
    assert_eq!(record.round, 1);
    assert_eq!(record.stopped, Some(record::Stopped::RoundsExhausted));
    // A wrong claim isn't silently dropped (decision 177(e)): it's why no-progress didn't fire.
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("M-1"), "{:?}", report.warnings);
}

#[test]
fn a_repeat_claim_naming_an_unknown_id_does_not_stop_the_task_and_warns() {
    let f = config_with_fix_rounds(1);
    let backend = backend(&f, &[FINDING1, REPEAT_UNKNOWN_ID]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(report.must_fix_left, 1);
    let record = saved(&f);
    assert_eq!(record.stopped, Some(record::Stopped::RoundsExhausted));
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("M-9"), "{:?}", report.warnings);
}

#[test]
fn an_intervening_clean_narrow_review_does_not_hide_an_older_must_fix_finding() {
    // Round 1 (full) finds M-1 in a.txt; a fix round runs. Round 2 (narrow, scoped to that fix's
    // commits) finds nothing, so round 3 (full, confirmatory) runs next and finds the bug still
    // there, correctly naming it `Repeat of: M-1`. Matching only against the review right before
    // round 3 (round 2, which has no findings at all) would miss this: the no-progress rule must
    // match against every earlier round (decision 177(o)), and round 3's reviewer context must
    // include round 1's still-open finding, not only round 2's (empty) review.
    let f = config_with_fix_rounds(3);
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
        vec![
            FINDING1.to_owned(),
            CLEAN.to_owned(),
            REPEAT_SAME_FILE.to_owned(),
        ],
    );
    let review_clone = f.env.base_dir().join("tasks").join("issue-41-review");
    let seen_previous = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = seen_previous.clone();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            if sandbox.contains("-review-")
                && spec.argv.iter().any(|a| a == "codex")
                && let Ok(text) =
                    fs::read_to_string(review_clone.join(".sbxm-task").join("previous-review.md"))
            {
                seen.lock().unwrap().push(text);
            }
            reviewer(sandbox, spec);
        });
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(
        report.rounds.iter().map(|r| r.must_fix).collect::<Vec<_>>(),
        [1, 0, 1]
    );
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(
        record.round, 1,
        "only the fix round after round 1 ran; round 3's repeat stopped the task before a second"
    );
    assert_eq!(record.stopped, Some(record::Stopped::RepeatFinding));
    assert_eq!(count(&backend, "codex"), 3, "no fourth review");

    let seen_previous = seen_previous.lock().unwrap();
    assert_eq!(seen_previous.len(), 2, "{seen_previous:?}");
    assert!(
        seen_previous[1].contains("a.txt does the wrong thing"),
        "round 3's context must include round 1's still-open finding, not only round 2's \
         (empty) review: {:?}",
        seen_previous[1]
    );
}

#[test]
fn failing_gates_before_the_review_stop_it_before_any_reviewer_exists() {
    // No fix rounds to spend: the first gate failure stops the task at once.
    let f = config_with_fix_rounds(0);
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
fn a_gate_failure_before_any_review_feeds_a_fix_round_with_the_gates_own_output() {
    let f = config();
    // Execs that no scripted rule matches are answered from this queue, in order: the worker's
    // bundle and status, the gates before the review (fail), the fix round's bundle and status,
    // then the gates after the fix round (pass).
    let pass = || ok("");
    let red = ExecOutput {
        stdout: String::new(),
        stderr: "undefined reference to `oops`\n".into(),
        exit_code: Some(101),
    };
    let backend =
        backend(&f, &[CLEAN]).with_exec_outputs(vec![pass(), pass(), red, pass(), pass(), pass()]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert!(report.gates_failed.is_none(), "the retry passed");
    assert_eq!(
        report.rounds.iter().map(|r| r.must_fix).collect::<Vec<_>>(),
        [0]
    );
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.round, 1, "the gate failure spent the only round");
    assert!(record.stopped.is_none());

    // The fix round used the gate's own prompt (no review.md existed yet) and named its output.
    let fix_execs: Vec<_> = backend
        .execs()
        .into_iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains("fix-prompt.md")))
        .collect();
    assert_eq!(fix_execs.len(), 1);
    let fix_prompt = fs::read_to_string(meta(&f).join("fix-prompt.md")).unwrap();
    assert!(
        fix_prompt.contains("gate-failure.md") && fix_prompt.contains("#41"),
        "{fix_prompt}"
    );
    assert!(
        !fix_prompt.to_lowercase().contains("reviewer checked"),
        "a gate-triggered fix round isn't told a reviewer ran: {fix_prompt}"
    );
    let workspace = f
        .env
        .base_dir()
        .join("tasks")
        .join("issue-41")
        .join(".sbxm-task");
    let gate_output = fs::read_to_string(workspace.join("gate-failure.md")).unwrap();
    assert!(
        gate_output.contains("cargo test") && gate_output.contains("oops"),
        "{gate_output}"
    );
    assert!(
        !workspace.join("review.md").exists(),
        "no review exists yet"
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
    assert_eq!(record.round, 1);
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

#[test]
fn a_worker_that_links_its_agent_folder_out_makes_the_fix_round_refuse_and_writes_nothing_there() {
    let f = config();
    let backend = backend(&f, &[ONE, CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    // After the worker is done it replaces `.sbxm-task` with a link to somewhere else on the host.
    let outside = f.env.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("review.md"), "precious").unwrap();
    let agent_dir = f
        .env
        .base_dir()
        .join("tasks")
        .join("issue-41")
        .join(".sbxm-task");
    fs::remove_dir_all(&agent_dir).unwrap();
    common::dir_link(&agent_dir, &outside);

    let _ = review(&f, &backend, &mut prepared);

    assert_eq!(fs::read(outside.join("review.md")).unwrap(), b"precious");
    assert!(!outside.join("fix-prompt.md").exists());
    assert_eq!(count(&backend, "fix-prompt.md"), 0, "no fix run");
}

#[test]
fn the_record_names_the_reviewers_sandbox_before_it_is_created() {
    let f = config();
    let meta = meta(&f);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let in_hook = seen.clone();
    let backend = backend(&f, &[CLEAN]).with_create_hook(move |spec| {
        if spec.name.contains("-review-") {
            let named = record::read(&meta.join("task.json"))
                .ok()
                .and_then(|r| r.reviewer)
                .map(|a| a.sandbox);
            in_hook.lock().unwrap().push((spec.name.clone(), named));
        }
    });
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].1.as_deref(),
        Some(seen[0].0.as_str()),
        "a kill during the multi-minute create must leave a record `task rm` can use"
    );
}

// ---- partial gate runs (PR 57 review round 2, M-1) ----

fn both_tiers() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\
         host = [\"cargo build\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

fn gates(
    f: &Fixture,
    backend: &FakeBackend,
    host: &FakeHostRunner,
    prepared: &mut Prepared,
    tiers: Tiers,
) -> anyhow::Result<pipeline::Gated> {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::run_gates(
        &ctx_with_host(f, &source, backend, &github, host).env(),
        prepared,
        "on-demand",
        tiers,
    )
}

fn review_with_host(
    f: &Fixture,
    backend: &FakeBackend,
    host: &FakeHostRunner,
    prepared: &mut Prepared,
) -> anyhow::Result<ReviewReport> {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::review_issue(
        &ctx_with_host(f, &source, backend, &github, host).env(),
        prepared,
    )
}

#[test]
fn a_sandbox_only_gate_run_does_not_authorize_review_without_the_host_tier() {
    let f = both_tiers();
    let backend = backend(&f, &[CLEAN]);
    let host = FakeHostRunner::default();
    let mut prepared = worked_task(&f, &backend);
    gates(&f, &backend, &host, &mut prepared, Tiers::SANDBOX).unwrap();
    assert!(host.calls().is_empty());

    review_with_host(&f, &backend, &host, &mut prepared).unwrap();

    assert_eq!(
        host.calls().len(),
        1,
        "review must run the configured host gate"
    );
}

#[test]
fn a_host_only_run_without_a_sandbox_pass_is_refused_and_runs_nothing() {
    let f = both_tiers();
    let backend = backend(&f, &[CLEAN]);
    let host = FakeHostRunner::default();
    let mut prepared = worked_task(&f, &backend);

    let err = gates(&f, &backend, &host, &mut prepared, Tiers::HOST).unwrap_err();

    assert!(format!("{err:#}").contains("sandbox"), "{err:#}");
    assert!(host.calls().is_empty(), "no agent code on the host");
}

#[test]
fn sandbox_then_host_on_the_same_commit_together_authorize_review() {
    let f = both_tiers();
    let backend = backend(&f, &[CLEAN]);
    let host = FakeHostRunner::default();
    let mut prepared = worked_task(&f, &backend);
    gates(&f, &backend, &host, &mut prepared, Tiers::SANDBOX).unwrap();
    gates(&f, &backend, &host, &mut prepared, Tiers::HOST).unwrap();
    assert_eq!(host.calls().len(), 1);
    let sandbox_runs = count(&backend, "cargo test");

    review_with_host(&f, &backend, &host, &mut prepared).unwrap();

    assert_eq!(
        host.calls().len(),
        1,
        "both tiers already passed on this commit"
    );
    assert_eq!(count(&backend, "cargo test"), sandbox_runs);
}

#[test]
fn a_new_commit_after_the_gates_means_review_runs_them_again() {
    let f = both_tiers();
    let backend = backend(&f, &[CLEAN]);
    let host = FakeHostRunner::default();
    let mut prepared = worked_task(&f, &backend);
    gates(&f, &backend, &host, &mut prepared, Tiers::ALL).unwrap();
    // The task branch moves on after the gates passed.
    let repo_git = meta(&f).join("repo.git");
    let tree = common::git(&repo_git, &["rev-parse", "issue-41^{tree}"]);
    let parent = common::git(&repo_git, &["rev-parse", "issue-41"]);
    let moved = common::git(
        &repo_git,
        &["commit-tree", &tree, "-p", &parent, "-m", "moved"],
    );
    common::git(&repo_git, &["update-ref", "refs/heads/issue-41", &moved]);

    review_with_host(&f, &backend, &host, &mut prepared).unwrap();

    assert_eq!(host.calls().len(), 2, "gates passed on an older commit");
}

fn head_of_main(f: &Fixture) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "refs/heads/main"])
        .current_dir(meta(f).join("repo.git"))
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn an_ordinary_tasks_reviewer_is_scoped_from_its_base_branch() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    let prompt = fs::read_to_string(meta(&f).join("reviewer-prompt-1.md")).unwrap();
    assert!(
        prompt.contains("git log origin/main..HEAD")
            && prompt.contains("git diff origin/main...HEAD"),
        "{prompt}"
    );
}

#[test]
fn a_continued_tasks_reviewer_is_scoped_from_the_pr_head_it_started_at() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    let head = head_of_main(&f);
    prepared.record.continues = Some(record::PrBranch {
        pr: 7,
        branch: "issue-41".into(),
        base: head.clone(),
    });

    review(&f, &backend, &mut prepared).unwrap();

    let prompt = fs::read_to_string(meta(&f).join("reviewer-prompt-1.md")).unwrap();
    assert!(
        prompt.contains(&format!("git log {head}..HEAD"))
            && prompt.contains(&format!("git diff {head}...HEAD")),
        "{prompt}"
    );
    assert!(!prompt.contains("origin/main"), "{prompt}");
}

#[test]
fn a_narrow_review_that_repeats_one_finding_keeps_the_full_reviews_other_findings_open() {
    // Review M-1 (issue 120, decision 177(f, p)): round 1 (full) finds M-1 and M-2, round 2
    // (narrow) repeats only M-1 and stops the task. M-2 was never seen fixed, so it stays open in
    // the record, next to round 2's M-1, and `finish` lists both from there.
    let f = config_with_fix_rounds(3);
    let backend = backend(&f, &[FINDINGS_TWO, REPEAT_SAME_FILE]);
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    let record = saved(&f);
    assert_eq!(record.stopped, Some(record::Stopped::RepeatFinding));
    let open = record
        .open_findings
        .expect("the open findings are recorded");
    let listed: Vec<_> = open
        .iter()
        .map(|o| (o.round, o.id.as_str(), o.title.as_str(), o.place.as_deref()))
        .collect();
    assert_eq!(
        listed,
        [
            (1, "M-2", "b.txt is wrong too", Some("`b.txt:3`")),
            (
                2,
                "M-1",
                "a.txt still does the wrong thing",
                Some("`a.txt:5`")
            ),
        ]
    );
}

#[test]
fn a_full_review_replaces_the_open_findings() {
    // A full review sees every commit, so what it finds is all that is open: round 1's M-2 is
    // gone once round 3 (the confirmatory full review after a clean narrow one) doesn't name it.
    let f = config_with_fix_rounds(3);
    let backend = backend(&f, &[FINDINGS_TWO, CLEAN, FINDING1, CLEAN, CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    let record = saved(&f);
    assert_eq!(record.stopped, None);
    assert_eq!(record.open_findings, Some(Vec::new()));
}

#[test]
fn a_review_whose_count_differs_from_its_must_fix_findings_is_not_used() {
    // Review round 6, M-2: a count with no `### M-…` findings under `## Must fix` would leave a
    // stopped task with nothing to list in its draft PR, so the review is refused like one with no
    // count line.
    let f = config();
    let backend = backend(&f, &["Must-fix findings: 2\n\nTwo things are wrong.\n"]);
    let mut prepared = worked_task(&f, &backend);

    let err = review(&f, &backend, &mut prepared).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("says 2 must-fix finding(s) but lists 0"),
        "{message}"
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Failed)
    );
    assert_eq!(record.open_findings, None);
    assert!(!meta(&f).join("review.md").exists());
    assert!(meta(&f).join("review-1.md").exists());
}
