//! `sbxm status`'s TIME column for a task that has ended (issue 484): a finished task, or one
//! that stopped at `ready` (`stopped` set), shows its total run time, from its first `prepared`
//! stamp to the stamp of the stage it ended in, fixed whatever the clock says; a task still going
//! shows the time in its current stage, as before.

use std::path::Path;

use sbxm::run::results::format_timestamp;
use sbxm::status::tasks::{self, TaskRow, TaskTime};
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Stamp, Status, Stopped,
};

const T0: u64 = 1_790_000_000;
const DAY: u64 = 86_400;

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// A task whose `stages` are `stamps` (stage, seconds after `T0`), in stage `stage` with
/// `status`.
fn task(stage: Stage, status: Status, stamps: &[(Stage, u64)]) -> Record {
    let mut record = Record::new(
        &NewTask {
            kind: Kind::Issue,
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
    );
    record.stage = stage;
    record.status = status;
    record.stages = stamps
        .iter()
        .map(|&(stage, after)| Stamp {
            stage,
            at: format_timestamp(T0 + after),
        })
        .collect();
    record
}

fn time_of(record: &Record) -> TaskTime {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), record);
    let rows = tasks::rows(base.path(), &Probe).unwrap();
    rows[0].time
}

fn save(base: &Path, record: &Record) {
    record::write(&record::task_dir(base, &record.id), record).unwrap();
}

fn row(time: TaskTime) -> TaskRow {
    TaskRow {
        id: "issue-7".to_owned(),
        title: String::new(),
        stage: "finished",
        status: "ok".to_owned(),
        time,
        commits_ahead: None,
        must_fix: None,
        pr: None,
        log_line: None,
    }
}

/// The TIME cell of `row`'s line in a render at `now`.
fn time_cell(row: &TaskRow, now: u64) -> String {
    let text = tasks::render(std::slice::from_ref(row), now, None);
    let line = text.lines().nth(1).unwrap().to_owned();
    line.split_whitespace().nth(3).unwrap().to_owned()
}

// ---- gathering: which time a row gets ---------------------------------------

#[test]
fn a_finished_task_takes_finished_at_minus_prepared_at() {
    let record = task(
        Stage::Finished,
        Status::Ok,
        &[
            (Stage::Prepared, 0),
            (Stage::Working, 5),
            (Stage::Gating, 600),
            (Stage::Reviewing, 700),
            (Stage::Ready, 1_000),
            (Stage::Finished, 1_200),
        ],
    );
    assert_eq!(time_of(&record), TaskTime::Took(Some(1_200)));
}

#[test]
fn a_task_finished_at_its_prepared_time_took_zero_seconds() {
    let record = task(
        Stage::Finished,
        Status::Ok,
        &[(Stage::Prepared, 0), (Stage::Finished, 0)],
    );
    assert_eq!(time_of(&record), TaskTime::Took(Some(0)));
}

#[test]
fn a_resumed_task_counts_from_its_first_prepared_stamp() {
    let record = task(
        Stage::Finished,
        Status::Ok,
        &[
            (Stage::Prepared, 0),
            (Stage::Working, 10),
            (Stage::Gating, 100),
            (Stage::Reviewing, 200),
            (Stage::Fixing, 300),
            (Stage::Gating, 400),
            (Stage::Reviewing, 500),
            (Stage::Reviewing, 600),
            (Stage::Ready, 700),
            (Stage::Finished, 2 * DAY + 3 * 3_600),
        ],
    );
    assert_eq!(time_of(&record), TaskTime::Took(Some(2 * DAY + 3 * 3_600)));
}

#[test]
fn a_task_stopped_at_ready_takes_its_ready_stamp_as_the_end() {
    let mut record = task(
        Stage::Ready,
        Status::Ok,
        &[
            (Stage::Prepared, 0),
            (Stage::Working, 10),
            (Stage::Reviewing, 300),
            (Stage::Ready, 900),
        ],
    );
    for stopped in [Stopped::RoundsExhausted, Stopped::RepeatFinding] {
        record.stopped = Some(stopped);
        assert_eq!(time_of(&record), TaskTime::Took(Some(900)), "{stopped:?}");
    }
}

#[test]
fn a_ready_task_that_did_not_stop_keeps_its_time_in_stage() {
    let record = task(
        Stage::Ready,
        Status::Ok,
        &[(Stage::Prepared, 0), (Stage::Ready, 900)],
    );
    assert_eq!(time_of(&record), TaskTime::InStage(Some(T0 + 900)));
}

#[test]
fn a_running_task_keeps_its_time_in_stage() {
    let record = task(
        Stage::Reviewing,
        Status::Running,
        &[
            (Stage::Prepared, 0),
            (Stage::Working, 10),
            (Stage::Reviewing, 300),
        ],
    );
    assert_eq!(time_of(&record), TaskTime::InStage(Some(T0 + 300)));
}

#[test]
fn a_finished_record_without_a_prepared_stamp_has_no_total() {
    let record = task(Stage::Finished, Status::Ok, &[(Stage::Finished, 500)]);
    assert_eq!(time_of(&record), TaskTime::Took(None));
}

#[test]
fn a_finished_record_with_an_unreadable_stamp_has_no_total() {
    let mut record = task(
        Stage::Finished,
        Status::Ok,
        &[(Stage::Prepared, 0), (Stage::Finished, 500)],
    );
    record.stages[0].at = "not a time".to_owned();
    assert_eq!(time_of(&record), TaskTime::Took(None));
    record.stages[0].at = format_timestamp(T0);
    record.stages[1].at = "not a time".to_owned();
    assert_eq!(time_of(&record), TaskTime::Took(None));
}

#[test]
fn a_finished_stamp_before_prepared_never_underflows() {
    let mut record = task(
        Stage::Finished,
        Status::Ok,
        &[(Stage::Prepared, 500), (Stage::Finished, 0)],
    );
    record.stages[1].at = format_timestamp(T0);
    assert_eq!(time_of(&record), TaskTime::Took(Some(0)));
}

// ---- rendering --------------------------------------------------------------

#[test]
fn a_total_stays_the_same_between_renders_at_different_times() {
    let r = row(TaskTime::Took(Some(1_200)));
    assert_eq!(time_cell(&r, T0), "20m");
    assert_eq!(time_cell(&r, T0 + 5 * DAY), "20m");
}

#[test]
fn a_total_of_zero_and_one_over_a_day_use_the_duration_format() {
    assert_eq!(time_cell(&row(TaskTime::Took(Some(0))), T0), "0s");
    assert_eq!(
        time_cell(&row(TaskTime::Took(Some(2 * DAY + 3 * 3_600))), T0),
        "2d03h"
    );
}

#[test]
fn a_missing_total_shows_a_dash() {
    assert_eq!(time_cell(&row(TaskTime::Took(None)), T0), "-");
}

#[test]
fn a_time_in_stage_grows_with_now() {
    let r = row(TaskTime::InStage(Some(T0)));
    assert_eq!(time_cell(&r, T0 + 60), "1m");
    assert_eq!(time_cell(&r, T0 + 120), "2m");
}

#[test]
fn a_time_in_stage_with_now_before_it_never_underflows() {
    let r = row(TaskTime::InStage(Some(T0)));
    assert_eq!(time_cell(&r, T0 - 100), "0s");
}
