//! Issue 513: the rules that used to live outside the state table (the recorded-PR rule of
//! `task finish`, the process check of `task resume`, and `task resume --rounds`) are rows of
//! `machine::TABLE` now, so `task states` lists them and the checks decide through them. The walk
//! over every (command, state) pair is `tests/main/task_machine.rs`; these are the boundary cases.

use sbxm::commands::task_states;
use sbxm::task::finish::{check_can_finish, check_can_retry_summaries};
use sbxm::task::machine::{Event, State, verdict};
use sbxm::task::record::{Kind, NewTask, Process, ProcessProbe, Record, Stage, Status, Stopped};
use sbxm::task::resume::check_can_resume;

const T0: u64 = 1_790_000_000;
const PR_URL: &str = "https://github.com/o/r/pull/9";

/// `Some(T0)`: the recorded process (started at `T0`) is alive; `None`: it is gone.
struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

const GONE: Probe = Probe(None);
const ALIVE: Probe = Probe(Some(T0));

fn task(stage: Stage, status: Status) -> Record {
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
        Process::new(1234, T0),
    );
    record.stage = stage;
    record.status = status;
    record
}

fn message(result: anyhow::Result<()>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn finish_without_a_recorded_pr_is_allowed_by_the_table() {
    let record = task(Stage::Ready, Status::Ok);
    check_can_finish(&record, &GONE).unwrap();
    assert!(verdict(&State::of(&record, &GONE), Event::Finish));
}

#[test]
fn finish_with_a_recorded_pr_is_refused_by_the_table_with_the_same_message() {
    let mut record = task(Stage::Ready, Status::Ok);
    record.pr = Some(PR_URL.to_owned());
    assert!(!verdict(&State::of(&record, &GONE), Event::Finish));
    assert_eq!(
        message(check_can_finish(&record, &GONE)),
        format!("task issue-7 already has a PR, {PR_URL}; there is nothing left to finish")
    );
}

#[test]
fn the_summary_retry_needs_a_recorded_pr_in_the_table() {
    let mut record = task(Stage::Finished, Status::Ok);
    assert!(!verdict(&State::of(&record, &GONE), Event::FinishSummaries));
    assert_eq!(
        message(check_can_retry_summaries(&record, &GONE)),
        "task issue-7 has no recorded PR, so there are no summaries to retry"
    );
    record.pr = Some(PR_URL.to_owned());
    assert!(verdict(&State::of(&record, &GONE), Event::FinishSummaries));
    check_can_retry_summaries(&record, &GONE).unwrap();
}

#[test]
fn resume_rounds_on_a_stopped_ready_task_is_allowed_by_the_table() {
    let mut record = task(Stage::Ready, Status::Ok);
    record.stopped = Some(Stopped::RoundsExhausted);
    assert!(verdict(&State::of(&record, &GONE), Event::ResumeRounds));
    check_can_resume(&record, &GONE, Some(2)).unwrap();
}

#[test]
fn resume_rounds_on_a_task_that_is_not_ready_is_refused_by_the_table_with_the_same_message() {
    let record = task(Stage::Working, Status::Failed);
    assert!(!verdict(&State::of(&record, &GONE), Event::ResumeRounds));
    // Without `--rounds` the same task resumes.
    assert!(verdict(&State::of(&record, &GONE), Event::Resume));
    assert_eq!(
        message(check_can_resume(&record, &GONE, Some(2))),
        "--rounds 2 is for a task that stopped ready with its rounds used; task issue-7 is at \
         working (failed), so resume it without --rounds"
    );
}

#[test]
fn resume_of_a_task_whose_recorded_process_is_alive_is_refused_by_the_table() {
    // Not `running`, so not interrupted either: only the process check refuses it.
    let record = task(Stage::Working, Status::Failed);
    assert!(!verdict(&State::of(&record, &ALIVE), Event::Resume));
    assert!(!verdict(&State::of(&record, &ALIVE), Event::ResumeRounds));
    assert_eq!(
        message(check_can_resume(&record, &ALIVE, None)),
        format!(
            "task issue-7 is still running in sbxm process pid 1234 (started {}); wait for it to \
             finish, or stop that process first",
            record.process.as_ref().unwrap().started_at
        )
    );
}

#[test]
fn state_of_reads_the_record_and_the_probe() {
    let mut record = task(Stage::Gating, Status::Running);
    let alive = State::of(&record, &ALIVE);
    assert_eq!(
        (alive.stage, alive.status, alive.kind),
        (Stage::Gating, Status::Running, Kind::Issue)
    );
    assert!(alive.alive && !alive.interrupted && !alive.has_pr);
    let gone = State::of(&record, &GONE);
    assert!(!gone.alive && gone.interrupted);
    record.pr = Some(PR_URL.to_owned());
    record.status = Status::Passed;
    let done = State::of(&record, &ALIVE);
    // Only a `running` task is interrupted; a live process of any status is `alive`.
    assert!(done.alive && !done.interrupted && done.has_pr);
}

#[test]
fn task_states_lists_the_moved_rules() {
    let text = task_states::render();
    let row = |from: &str, event: &str| {
        text.lines()
            .find(|l| l.starts_with(&format!("| {from} | {event} |")))
            .unwrap_or_else(|| panic!("no row `{from} | {event}` in:\n{text}"))
            .to_owned()
    };
    row("ready ok (issue), no recorded PR", "task finish");
    row(
        "finished ok (issue/spec), with a recorded PR",
        "task finish",
    );
    row(
        "ready ok (issue/spec/fix), no live process",
        "task resume --rounds N",
    );
    row("ready ok (issue/spec/fix), no live process", "task resume");
    row(
        "working failed (issue/spec/fix), no live process",
        "task resume",
    );
}
