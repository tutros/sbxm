//! Issue 647: the no-progress rule (spec §5.3, issue 119) tolerates a task's first
//! validated repeat once, while the fix-round budget has a round left: one more fix round, the
//! gates and the next review run as for a new finding. A second repeat stops the task
//! `repeat-finding`; a repeat with no budget left stops it `rounds-exhausted`.

use crate::common;

use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Prepared, ReviewReport, TaskEnv};
use sbxm::task::record::{self, ProcessProbe, ReviewResult, Stage, Status, Stopped};
use sbxm::task::{repo, resume};

fn config_with_fix_rounds(fix_rounds: u32) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = {fix_rounds}\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    ))
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

/// A structured must-fix finding in `a.txt`, so a later round can validly repeat it.
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

/// A new must-fix finding in another file: not a repeat of anything.
const OTHER: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - b.txt is wrong\n\n\
    **Where:** `b.txt:3`\n\
    **What happens:** it is wrong\n\
    **Why it matters:** decision 2\n\
    **Fix:** make it right\n";

/// The process a record names is gone.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// The worker (and each fix round) commits `a.txt`; the n-th review is `reviews[n]`.
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

fn env<'a>(f: &'a Fixture, backend: &'a FakeBackend, progress: &'a dyn Fn(&str)) -> TaskEnv<'a> {
    let host: &'static FakeHostRunner = Box::leak(Box::default());
    TaskEnv {
        progress,
        config_dir: Box::leak(Box::new(f.env.config_dir())),
        config: &f.config,
        backend,
        probe: &Gone,
        host,
        sleep: &std::thread::sleep,
        scoped_secrets: &[],
        disk: Box::leak(Box::default()),
        identity: None,
        clone_source: None,
        hold: None,
    }
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

fn fix_rounds_run(backend: &FakeBackend) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains("fix-prompt.md")))
        .count()
}

/// Reviews issue 41 through `reviews`, returning the report and every progress line.
fn review(f: &Fixture, reviews: &[&str]) -> (ReviewReport, Vec<String>, FakeBackend) {
    let backend = backend(f, reviews);
    let mut prepared = worked_task(f, &backend);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let seen = lines.clone();
    let progress = move |line: &str| seen.lock().unwrap().push(line.to_owned());
    let report = pipeline::review_issue(&env(f, &backend, &progress), &mut prepared).unwrap();
    let lines = lines.lock().unwrap().clone();
    (report, lines, backend)
}

fn rounds(report: &ReviewReport) -> Vec<u32> {
    report.rounds.iter().map(|r| r.must_fix).collect()
}

#[test]
fn review_repeat_tolerated_a_first_repeat_with_budget_left_gets_one_more_fix_round_then_ready() {
    // Case 1: round 1 finds M-1, round 2 repeats it with budget left: a fix round runs, round 3
    // (narrow) and round 4 (full) are clean.
    let f = config_with_fix_rounds(3);
    let (report, lines, backend) = review(&f, &[FINDING1, REPEAT, CLEAN, CLEAN]);

    assert_eq!(rounds(&report), [1, 1, 0, 0]);
    assert_eq!(fix_rounds_run(&backend), 2);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(record.stopped, None);
    assert_eq!(record.round, 2, "the extra round counts against the budget");
    assert!(record.repeat_tolerated);
    let said: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("tolerated once:"))
        .collect();
    assert_eq!(said.len(), 1, "{lines:?}");
    assert!(said[0].starts_with("issue-41: "), "{said:?}");
    assert!(said[0].contains("one more fix round"), "{said:?}");
}

#[test]
fn review_repeat_tolerated_a_second_repeat_stops_the_task_repeat_finding() {
    // Case 2: round 3 repeats again after the tolerated round.
    let f = config_with_fix_rounds(3);
    let (report, _, backend) = review(&f, &[FINDING1, REPEAT, REPEAT]);

    assert_eq!(rounds(&report), [1, 1, 1]);
    assert_eq!(fix_rounds_run(&backend), 2);
    let record = saved(&f);
    assert_eq!(record.stage, Stage::Ready);
    assert_eq!(record.round, 2, "the budget had a round left, unused");
    assert_eq!(record.stopped, Some(Stopped::RepeatFinding));
    assert!(record.repeat_tolerated);
}

#[test]
fn review_repeat_tolerated_a_repeat_with_the_budget_used_stops_rounds_exhausted_untolerated() {
    // Case 3: round 1 finds M-1, round 2 a new finding, round 3 repeats M-1 at round ==
    // fix_rounds.
    let f = config_with_fix_rounds(2);
    let (report, lines, backend) = review(&f, &[FINDING1, OTHER, REPEAT]);

    assert_eq!(rounds(&report), [1, 1, 1]);
    assert_eq!(fix_rounds_run(&backend), 2);
    let record = saved(&f);
    assert_eq!(record.round, 2);
    assert_eq!(record.stopped, Some(Stopped::RoundsExhausted));
    assert!(!record.repeat_tolerated);
    assert!(!lines.iter().any(|l| l.contains("tolerated")), "{lines:?}");
}

#[test]
fn review_repeat_tolerated_once_per_task_not_per_finding() {
    // Case 4: a tolerated repeat (round 2), a new finding (round 3), then a repeat (round 4):
    // the second repeat stops the task though budget is left.
    let f = config_with_fix_rounds(4);
    let (report, _, backend) = review(&f, &[FINDING1, REPEAT, OTHER, REPEAT]);

    assert_eq!(rounds(&report), [1, 1, 1, 1]);
    assert_eq!(fix_rounds_run(&backend), 3);
    let record = saved(&f);
    assert_eq!(record.round, 3);
    assert_eq!(record.stopped, Some(Stopped::RepeatFinding));
    assert!(record.repeat_tolerated);
}

#[test]
fn review_repeat_tolerated_a_budget_of_one_stops_the_repeat_rounds_exhausted() {
    // Case 5: fix_rounds = 1: round 1 must-fix, the fix round, round 2 repeats: no budget left.
    let f = config_with_fix_rounds(1);
    let (report, _, backend) = review(&f, &[FINDING1, REPEAT]);

    assert_eq!(rounds(&report), [1, 1]);
    assert_eq!(fix_rounds_run(&backend), 1);
    let record = saved(&f);
    assert_eq!(record.round, 1);
    assert_eq!(record.stopped, Some(Stopped::RoundsExhausted));
    assert!(!record.repeat_tolerated);
}

#[test]
fn review_repeat_tolerated_an_older_record_without_the_field_reads_as_not_tolerated() {
    // Case 6: a record written before this change has no `repeat_tolerated`.
    let f = config_with_fix_rounds(1);
    let backend = backend(&f, &[CLEAN]);
    worked_task(&f, &backend);
    let path = meta(&f).join("task.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("repeat_tolerated");
    std::fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

    assert!(!record::read(&path).unwrap().repeat_tolerated);
}

#[test]
fn review_repeat_tolerated_is_written_only_once_used() {
    let f = config_with_fix_rounds(1);
    let backend = backend(&f, &[CLEAN]);
    worked_task(&f, &backend);
    let text = std::fs::read_to_string(meta(&f).join("task.json")).unwrap();

    assert!(!text.contains("repeat_tolerated"), "{text}");
}

#[test]
fn review_repeat_tolerated_resume_of_an_older_repeat_stop_ignores_one_then_tolerates_one() {
    // Case 7: a task stopped `repeat-finding` before this change (after 1 fix round, budget 5,
    // nothing tolerated). The resumed fix round's review ignores its repeat (`ignore_repeat`,
    // without using the tolerance); the next repeat is tolerated; then two clean reviews.
    let f = config_with_fix_rounds(1);
    let first = backend(&f, &[FINDING1, REPEAT]);
    let mut prepared: Prepared = worked_task(&f, &first);
    pipeline::review_issue(&env(&f, &first, &pipeline::quiet), &mut prepared).unwrap();
    prepared.record.stopped = Some(Stopped::RepeatFinding);
    prepared.record.fix_rounds = 5;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let backend = backend(&f, &[REPEAT, REPEAT, CLEAN, CLEAN]);

    let resumed =
        resume::resume(&env(&f, &backend, &pipeline::quiet), &mut prepared, None).unwrap();

    let report = resumed.review.unwrap();
    assert_eq!(
        report.rounds.iter().map(|r| r.round).collect::<Vec<_>>(),
        [3, 4, 5, 6]
    );
    // The resumed round, the one after the ignored repeat, the one after the tolerated repeat.
    assert_eq!(fix_rounds_run(&backend), 3);
    let record = saved(&f);
    assert_eq!((record.stage, record.stopped), (Stage::Ready, None));
    assert_eq!(record.round, 4);
    assert!(record.repeat_tolerated);
}

#[test]
fn review_repeat_tolerated_a_replayed_repeat_with_budget_left_runs_its_fix_round() {
    // `task resume` of a review that completed with a repeat before the task acted on it: the
    // recorded result is replayed and tolerated like a fresh one.
    let f = config_with_fix_rounds(3);
    let worked = backend(&f, &[]);
    let mut prepared = worked_task(&f, &worked);
    prepared.record.round = 1;
    prepared.record.status = Status::Completed;
    prepared.record.stage = Stage::Reviewing;
    std::fs::write(meta(&f).join("review-1.md"), FINDING1).unwrap();
    std::fs::write(meta(&f).join("review-2.md"), REPEAT).unwrap();
    std::fs::write(meta(&f).join("review.md"), REPEAT).unwrap();
    prepared.record.last_reviewed_commit =
        Some(repo::branch_tip(&meta(&f).join("repo.git"), "issue-41").unwrap());
    prepared.record.review = Some(ReviewResult {
        round: 2,
        must_fix: 1,
        full: false,
        repeat: true,
        ..Default::default()
    });
    record::write(&prepared.meta, &prepared.record).unwrap();
    let backend = backend(&f, &[CLEAN, CLEAN]);

    resume::resume(&env(&f, &backend, &pipeline::quiet), &mut prepared, None).unwrap();

    assert_eq!(fix_rounds_run(&backend), 1);
    let record = saved(&f);
    assert_eq!((record.stage, record.stopped), (Stage::Ready, None));
    assert_eq!(record.round, 2);
    assert!(record.repeat_tolerated);
}
