//! Issue 513, second review M-1: whether a clean `ready` task may still have its should-fix round
//! (0 must-fix in its recorded review, and no such round yet) is part of a table state
//! (`machine::ShouldFixRound`), so `task resume --rounds 1 --should-fix` is refused by the table
//! itself for a task that already had its round or has must-fix findings left, with the messages
//! `check_should_fix` gives. What the review file holds stays `check_should_fix`'s.

use std::path::Path;

use sbxm::commands::task_states;
use sbxm::task::machine::{Event, SHOULD_FIX_ROUNDS, STOPS, ShouldFixRound, State, Stop, verdict};
use sbxm::task::record::{
    Kind, NewTask, Process, ProcessProbe, Record, ReviewResult, Stage, Status, Stopped,
};
use sbxm::task::resume::{check_can_resume_with, check_should_fix};

const T0: u64 = 1_790_000_000;

/// A review with one should-fix finding sized S and no must-fix.
const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";

/// The recorded process is gone.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn ready(kind: Kind) -> Record {
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
    record.stage = Stage::Ready;
    record.status = Status::Ok;
    record.fix_rounds = 2;
    record
}

fn with_must_fix(mut record: Record, must_fix: u32) -> Record {
    record.review = Some(ReviewResult {
        must_fix,
        full: true,
        ..ReviewResult::default()
    });
    record
}

/// A task folder holding [`S_ONE`] as its `review.md`.
fn meta() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("review.md"), S_ONE).unwrap();
    dir
}

fn table(record: &Record) -> bool {
    verdict(&State::of(record, &Gone), Event::ResumeShouldFix)
}

fn command(record: &Record, meta: &Path) -> anyhow::Result<()> {
    check_can_resume_with(record, &Gone, Some(1), true)?;
    check_should_fix(record, meta, Some(1)).map(|_| ())
}

fn refusal(result: anyhow::Result<()>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn state_of_reads_the_must_fix_count_and_the_used_round() {
    let round = |record: &Record| State::of(record, &Gone).should_fix;
    assert_eq!(round(&ready(Kind::Issue)), ShouldFixRound::Open);
    assert_eq!(
        round(&with_must_fix(ready(Kind::Issue), 0)),
        ShouldFixRound::Open
    );
    assert_eq!(
        round(&with_must_fix(ready(Kind::Issue), 1)),
        ShouldFixRound::MustFixLeft
    );
    let mut used = ready(Kind::Issue);
    used.should_fix_round = true;
    assert_eq!(round(&used), ShouldFixRound::Used);
    // Must-fix left is named first, as `check_should_fix` refuses it first.
    assert_eq!(round(&with_must_fix(used, 1)), ShouldFixRound::MustFixLeft);
}

#[test]
fn the_first_should_fix_round_is_allowed_and_a_second_refused_by_table_and_command() {
    let dir = meta();
    for kind in [Kind::Issue, Kind::Spec, Kind::Fix] {
        let mut record = with_must_fix(ready(kind), 0);
        assert!(table(&record), "{kind:?}");
        command(&record, dir.path()).unwrap();
        record.should_fix_round = true;
        assert!(!table(&record), "{kind:?}");
        let resume = refusal(check_can_resume_with(&record, &Gone, Some(1), true));
        assert!(
            resume.contains("already had its should-fix round, and a task gets at most one"),
            "{resume}"
        );
        // The same message as `check_should_fix`'s own.
        assert_eq!(
            resume,
            refusal(check_should_fix(&record, dir.path(), Some(1)).map(|_| ()))
        );
    }
    let mut record = ready(Kind::Issue);
    record.should_fix_round = true;
    assert_eq!(
        refusal(command(&record, dir.path())),
        "task issue-7 already had its should-fix round, and a task gets at most one; finish it \
         with `sbxm task finish --issue 7`, and what is left is filed with `sbxm task \
         file-findings`"
    );
}

#[test]
fn a_task_with_must_fix_left_is_refused_by_table_and_command() {
    let dir = meta();
    let record = with_must_fix(ready(Kind::Issue), 1);
    assert!(!table(&record));
    assert_eq!(
        refusal(check_can_resume_with(&record, &Gone, Some(1), true)),
        "--should-fix is for a clean task (0 must-fix, no stop); task issue-7 stopped ready (no \
         stop) with 1 must-fix finding(s), so resume it without --should-fix"
    );
    assert_eq!(
        refusal(check_should_fix(&record, dir.path(), Some(1)).map(|_| ())),
        refusal(check_can_resume_with(&record, &Gone, Some(1), true))
    );
}

/// `check_should_fix` is also asked on its own, by the loop (`should_fix_due`), with no process
/// probe: for every `ready` record it answers as the table does once the process is gone.
#[test]
fn check_should_fix_agrees_with_the_table_for_every_ready_record() {
    let dir = meta();
    for kind in [Kind::Issue, Kind::Spec, Kind::Fix] {
        for stop in STOPS {
            for round in SHOULD_FIX_ROUNDS {
                let mut record = ready(kind);
                (record.stopped, record.round) = match stop {
                    Stop::Clean => (None, 0),
                    Stop::RoundsLeft => (Some(Stopped::RepeatFinding), 1),
                    Stop::RoundsUsed => (Some(Stopped::RoundsExhausted), 2),
                };
                match round {
                    ShouldFixRound::Open => {}
                    ShouldFixRound::MustFixLeft => record = with_must_fix(record, 1),
                    ShouldFixRound::Used => record.should_fix_round = true,
                }
                let state = State::of(&record, &Gone);
                assert_eq!((state.stop, state.should_fix), (stop, round));
                assert_eq!(
                    table(&record),
                    check_should_fix(&record, dir.path(), Some(1)).is_ok(),
                    "{kind:?} {stop:?} {round:?}"
                );
            }
        }
    }
}

#[test]
fn task_states_lists_the_unused_should_fix_round() {
    let text = task_states::render();
    let row = text
        .lines()
        .find(|l| l.contains("| task resume --rounds 1 --should-fix |"))
        .unwrap_or_else(|| panic!("no should-fix row in:\n{text}"));
    assert!(
        row.starts_with(
            "| ready ok (issue/spec/fix), no live process, not stopped, no must-fix, \
             should-fix round unused |"
        ),
        "{row}"
    );
}
