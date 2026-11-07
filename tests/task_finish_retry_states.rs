//! The summary-only finish retry (issues 430, 467) in the state machine: `task states` documents
//! it as its own operation, and the check `task finish` makes before retrying agrees with it.

use sbxm::commands::task_states;
use sbxm::task::finish::check_can_retry_summaries;
use sbxm::task::machine::{Event, State, verdict};
use sbxm::task::record::{Kind, NewTask, Process, Record, Stage, Status};

const T0: u64 = 1_790_000_000;

fn finished(kind: Kind, pr: Option<&str>) -> Record {
    let id = match kind {
        Kind::Spec => Some("spec-t-abc123"),
        Kind::Fix => Some("fix-7-1234567890ab"),
        Kind::Issue | Kind::Pr => None,
    };
    let mut record = Record::new(
        &NewTask {
            kind,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id,
        },
        T0,
        Process::new(1234, T0),
    );
    record.stage = Stage::Finished;
    record.status = Status::Ok;
    record.pr = pr.map(str::to_owned);
    record
}

fn state(record: &Record) -> State {
    State {
        stage: record.stage,
        status: record.status,
        interrupted: false,
        kind: record.kind,
    }
}

#[test]
fn the_retry_agrees_with_the_table_for_every_kind() {
    for kind in [Kind::Issue, Kind::Spec, Kind::Fix, Kind::Pr] {
        let record = finished(kind, Some("https://github.com/o/r/pull/9"));
        assert_eq!(
            check_can_retry_summaries(&record).is_ok(),
            verdict(&state(&record), Event::FinishSummaries),
            "{kind:?}",
        );
    }
}

#[test]
fn a_finished_issue_or_spec_task_with_a_pr_may_retry() {
    for kind in [Kind::Issue, Kind::Spec] {
        let record = finished(kind, Some("https://github.com/o/r/pull/9"));
        check_can_retry_summaries(&record).unwrap();
    }
}

#[test]
fn a_finished_task_without_a_pr_or_a_fix_task_is_refused() {
    // A spec task under the `local` or `push` sink records no PR.
    for kind in [Kind::Issue, Kind::Spec] {
        assert!(check_can_retry_summaries(&finished(kind, None)).is_err());
    }
    for kind in [Kind::Fix, Kind::Pr] {
        let pr = Some("https://github.com/o/r/pull/9");
        assert!(check_can_retry_summaries(&finished(kind, pr)).is_err());
    }
}

#[test]
fn a_task_that_is_not_finished_is_refused() {
    let mut record = finished(Kind::Issue, Some("https://github.com/o/r/pull/9"));
    record.stage = Stage::Ready;
    assert!(check_can_retry_summaries(&record).is_err());
}

#[test]
fn task_states_documents_the_retry_without_a_publication() {
    let text = task_states::render();
    let row = text
        .lines()
        .find(|l| l.contains("finished ok (issue/spec)") && l.contains("task finish"))
        .expect("no summary-only retry row");
    assert!(row.contains("summaries only"), "{row}");
    assert!(row.contains("no push, no new PR, no PR review"), "{row}");
}
