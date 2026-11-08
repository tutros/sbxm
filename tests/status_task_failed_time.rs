//! A failed task's end time (issue 492): `task.json` records `ended_at` when a stage ends
//! `failed`, a stage that starts again clears it, and `sbxm status` shows a failed task's TIME as
//! `ended_at` minus its first `prepared` stamp, fixed whatever the clock says (`-` without it).

mod common;

use std::path::Path;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play_reviewer, play_tasks,
    worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::run::results::{format_timestamp, now};
use sbxm::status::tasks::{self, TaskRow, TaskTime};
use sbxm::task::pipeline::{self, Tiers};
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Stamp, Status,
};

const T0: u64 = 1_790_000_000;
const DAY: u64 = 86_400;

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn new_task(kind: Kind) -> Record {
    Record::new(
        &NewTask {
            kind,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

/// A failed task in `stage` whose `stages` are `stamps` (stage, seconds after `T0`), ended at
/// `ended` seconds after `T0` (`None`: an older record without `ended_at`).
fn failed(stage: Stage, stamps: &[(Stage, u64)], ended: Option<u64>) -> Record {
    let mut record = new_task(Kind::Issue);
    record.stage = stage;
    record.status = Status::Failed;
    record.stages = stamps
        .iter()
        .map(|&(stage, after)| Stamp {
            stage,
            at: format_timestamp(T0 + after),
        })
        .collect();
    record.ended_at = ended.map(|after| format_timestamp(T0 + after));
    record
}

fn time_of(record: &Record) -> TaskTime {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), record);
    tasks::rows(base.path(), &Probe).unwrap()[0].time
}

fn save(base: &Path, record: &Record) {
    record::write(&record::task_dir(base, &record.id), record).unwrap();
}

/// The TIME cell of a row with `time`, rendered at `now`.
fn time_cell(time: TaskTime, now: u64) -> String {
    let row = TaskRow {
        id: "issue-7".to_owned(),
        title: String::new(),
        stage: "working",
        status: "failed".to_owned(),
        time,
        commits_ahead: None,
        must_fix: None,
        pr: None,
        log_line: None,
    };
    let text = tasks::render(std::slice::from_ref(&row), now, None);
    let line = text.lines().nth(1).unwrap().to_owned();
    line.split_whitespace().nth(3).unwrap().to_owned()
}

// ---- the record: `ended_at` -------------------------------------------------

#[test]
fn a_stage_that_ends_failed_records_its_end_time_once() {
    let mut record = new_task(Kind::Issue);
    record
        .advance(Stage::Working, T0 + 5, Process::new(1, T0))
        .unwrap();
    record.finish_at(Status::Failed, T0 + 90).unwrap();

    assert_eq!(record.status, Status::Failed);
    assert_eq!(record.ended_at, Some(format_timestamp(T0 + 90)));
}

#[test]
fn a_stage_that_ends_any_other_way_records_no_end_time() {
    for status in [Status::Completed, Status::TimedOut] {
        let mut record = new_task(Kind::Issue);
        record
            .advance(Stage::Working, T0 + 5, Process::new(1, T0))
            .unwrap();
        record.finish_at(status, T0 + 90).unwrap();
        assert_eq!(record.ended_at, None, "{status:?}");
    }
}

#[test]
fn a_refused_status_records_no_end_time() {
    let mut record = new_task(Kind::Issue);
    // `ok` doesn't belong to `prepared`.
    assert!(record.finish_at(Status::Ok, T0 + 90).is_err());
    assert_eq!(
        (record.status, record.ended_at.as_deref()),
        (Status::Running, None)
    );
}

#[test]
fn finish_stamps_a_failure_with_the_clock() {
    let mut record = new_task(Kind::Issue);
    let before = now();
    record.finish(Status::Failed).unwrap();
    let after = now();

    let at = record
        .ended_at
        .as_deref()
        .and_then(sbxm::run::results::parse_timestamp)
        .expect("an end time");
    assert!(
        (before..=after).contains(&at),
        "{at} not in {before}..={after}"
    );
}

#[test]
fn a_rerun_after_a_failure_clears_the_end_time() {
    let mut record = new_task(Kind::Issue);
    record
        .advance(Stage::Working, T0 + 5, Process::new(1, T0))
        .unwrap();
    record.finish_at(Status::Failed, T0 + 90).unwrap();

    record.rerun(T0 + 200, Process::new(2, T0 + 200)).unwrap();

    assert_eq!(record.ended_at, None);
}

#[test]
fn a_review_begun_again_after_a_failed_one_clears_the_end_time() {
    let mut record = new_task(Kind::Issue);
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish_at(Status::Completed, T0 + 1).unwrap();
    record.begin_gating(T0 + 2, Process::new(1, T0)).unwrap();
    record.finish_at(Status::Passed, T0 + 3).unwrap();
    record.begin_review(T0 + 4, Process::new(1, T0)).unwrap();
    record.finish_at(Status::Failed, T0 + 5).unwrap();
    assert!(record.ended_at.is_some());

    record.begin_review(T0 + 6, Process::new(1, T0)).unwrap();

    assert_eq!(record.ended_at, None);
}

#[test]
fn a_pr_task_whose_gating_starts_over_after_a_failed_preparation_clears_the_end_time() {
    let mut record = new_task(Kind::Pr);
    record.finish_at(Status::Failed, T0 + 5).unwrap();
    assert!(record.ended_at.is_some());

    record
        .restart_pr_gating(T0 + 60, Process::new(2, T0 + 60))
        .unwrap();

    assert_eq!(record.ended_at, None);
}

#[test]
fn a_record_without_the_field_loads_and_one_without_an_end_time_writes_none() {
    let base = tempfile::tempdir().unwrap();
    let record = new_task(Kind::Issue);
    save(base.path(), &record);
    let path = record::task_dir(base.path(), &record.id).join("task.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("ended_at"), "{text}");

    let loaded = record::read(&path).unwrap();
    assert_eq!(loaded.ended_at, None);
}

#[test]
fn an_end_time_survives_a_write_and_a_read() {
    let base = tempfile::tempdir().unwrap();
    let record = failed(Stage::Working, &[(Stage::Prepared, 0)], Some(90));
    save(base.path(), &record);

    let path = record::task_dir(base.path(), &record.id).join("task.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(&format!("\"ended_at\": \"{}\"", format_timestamp(T0 + 90))),
        "{text}"
    );
    assert_eq!(record::read(&path).unwrap().ended_at, record.ended_at);
}

// ---- `sbxm status`: which time a failed task gets ---------------------------

#[test]
fn a_failed_task_takes_ended_at_minus_prepared_at() {
    let record = failed(
        Stage::Reviewing,
        &[
            (Stage::Prepared, 0),
            (Stage::Working, 10),
            (Stage::Gating, 600),
            (Stage::Reviewing, 700),
        ],
        Some(1_000),
    );
    assert_eq!(time_of(&record), TaskTime::Took(Some(1_000)));
}

#[test]
fn a_task_failed_at_prepared_at_once_took_zero_seconds() {
    let record = failed(Stage::Prepared, &[(Stage::Prepared, 0)], Some(0));
    assert_eq!(time_of(&record), TaskTime::Took(Some(0)));
}

#[test]
fn a_task_failed_after_a_day_took_over_a_day() {
    let record = failed(
        Stage::Working,
        &[(Stage::Prepared, 0), (Stage::Working, 10)],
        Some(DAY + 1),
    );
    assert_eq!(time_of(&record), TaskTime::Took(Some(DAY + 1)));
    assert_eq!(time_cell(time_of(&record), T0 + 5 * DAY), "1d00h");
}

#[test]
fn an_older_failed_task_without_an_end_time_has_no_total_and_shows_a_dash() {
    let record = failed(
        Stage::Working,
        &[(Stage::Prepared, 0), (Stage::Working, 10)],
        None,
    );
    assert_eq!(time_of(&record), TaskTime::Took(None));
    assert_eq!(time_cell(TaskTime::Took(None), T0 + 60), "-");
    assert_eq!(time_cell(TaskTime::Took(None), T0 + 120), "-");
}

#[test]
fn a_failed_task_with_an_unreadable_stamp_has_no_total() {
    let mut record = failed(Stage::Working, &[(Stage::Prepared, 0)], Some(90));
    record.ended_at = Some("not a time".to_owned());
    assert_eq!(time_of(&record), TaskTime::Took(None));
}

#[test]
fn an_end_time_before_prepared_never_underflows() {
    let record = failed(Stage::Working, &[(Stage::Prepared, 500)], Some(0));
    assert_eq!(time_of(&record), TaskTime::Took(Some(0)));
}

#[test]
fn a_failed_tasks_time_is_the_same_between_two_renders() {
    let time = time_of(&failed(
        Stage::Working,
        &[(Stage::Prepared, 0), (Stage::Working, 10)],
        Some(90),
    ));
    assert_eq!(time_cell(time, T0 + 100), "1m");
    assert_eq!(time_cell(time, T0 + 160), "1m");
}

#[test]
fn a_failed_task_resumed_then_finished_shows_its_new_total() {
    let mut record = failed(
        Stage::Working,
        &[(Stage::Prepared, 0), (Stage::Working, 10)],
        Some(90),
    );
    record.rerun(T0 + 500, Process::new(2, T0 + 500)).unwrap();
    // Running again: its time in the stage, not the old failure's total.
    assert_eq!(time_of(&record), TaskTime::InStage(Some(T0 + 500)));

    record.finish_at(Status::Completed, T0 + 600).unwrap();
    record.stage = Stage::Finished;
    record.status = Status::Ok;
    record.stages.push(Stamp {
        stage: Stage::Finished,
        at: format_timestamp(T0 + 2_000),
    });
    assert_eq!(time_of(&record), TaskTime::Took(Some(2_000)));
}

// ---- the pipeline: a failed review leaves its end time in `task.json` -------

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

#[test]
fn a_review_that_fails_saves_its_end_time_and_status_shows_a_fixed_total() {
    let f = config();
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    // A review without its count line isn't used: the round fails.
    let reviewer = play_reviewer(
        &f.env.base_dir(),
        "issue-41",
        Some("Looks fine to me.\n".to_owned()),
    );
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    let mut prepared = worked_task(&f, &backend);
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(&f);
    let ctx = ctx(&f, &source, &backend, &github);
    assert!(
        pipeline::run_gates(&ctx.env(), &mut prepared, "after-worker", Tiers::ALL)
            .unwrap()
            .passed
    );
    let before = now();
    assert!(pipeline::run_reviewer(&ctx.env(), &mut prepared, 1).is_err());
    let after = now();

    let saved =
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap();
    assert_eq!(saved.status, Status::Failed);
    let ended = saved
        .ended_at
        .as_deref()
        .and_then(sbxm::run::results::parse_timestamp)
        .expect("an end time");
    assert!((before..=after).contains(&ended));
    let prepared_at = sbxm::run::results::parse_timestamp(&saved.stages[0].at).unwrap();
    assert_eq!(
        tasks::rows(&f.env.base_dir(), &Probe).unwrap()[0].time,
        TaskTime::Took(Some(ended - prepared_at))
    );
}
