//! Issue 513, review M-1: whether a `ready` task stopped, and with fix rounds left or not, is part
//! of a table state (`machine::Stop`), so `task resume`, `task resume --rounds N` and `task resume
//! --rounds 1 --should-fix` on a `ready` task are decided by the table, with the same answers and
//! refusal messages as before. The walk over every (command, state) pair is
//! `tests/main/task_machine.rs`.

use sbxm::commands::task_states;
use sbxm::task::machine::{Event, State, Stop, verdict};
use sbxm::task::record::{Kind, NewTask, Process, ProcessProbe, Record, Stage, Status, Stopped};
use sbxm::task::resume::{check_can_resume, check_can_resume_with};

const T0: u64 = 1_790_000_000;

/// The recorded process is gone.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// A `ready` task of `kind` with `stopped`, `round` fix rounds used out of a budget of 2.
fn ready(kind: Kind, stopped: Option<Stopped>, round: u32) -> Record {
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
    record.stopped = stopped;
    record.round = round;
    record.fix_rounds = 2;
    record
}

fn table(record: &Record, event: Event) -> bool {
    verdict(&State::of(record, &Gone), event)
}

fn refusal(result: anyhow::Result<()>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn state_of_reads_the_stop_and_the_budget() {
    let stop = |stopped, round| State::of(&ready(Kind::Issue, stopped, round), &Gone).stop;
    assert_eq!(stop(None, 0), Stop::Clean);
    assert_eq!(stop(Some(Stopped::RepeatFinding), 1), Stop::RoundsLeft);
    // At the cap and one over it: no rounds left.
    assert_eq!(stop(Some(Stopped::RepeatFinding), 2), Stop::RoundsUsed);
    assert_eq!(stop(Some(Stopped::RepeatFinding), 3), Stop::RoundsUsed);
    assert_eq!(stop(Some(Stopped::RoundsExhausted), 0), Stop::RoundsUsed);
}

#[test]
fn a_clean_ready_issue_or_spec_task_is_refused_rounds_by_the_table_and_resume() {
    for kind in [Kind::Issue, Kind::Spec] {
        let record = ready(kind, None, 0);
        assert!(!table(&record, Event::ResumeRounds), "{kind:?}");
        assert!(!table(&record, Event::Resume), "{kind:?}");
        let message = refusal(check_can_resume(&record, &Gone, Some(1)));
        assert!(
            message.contains("did not stop on anything unresolved, so there is nothing to resume"),
            "{message}"
        );
        assert_eq!(
            refusal(check_can_resume(&record, &Gone, None)),
            message,
            "{kind:?}"
        );
    }
    assert_eq!(
        refusal(check_can_resume(
            &ready(Kind::Issue, None, 0),
            &Gone,
            Some(1)
        )),
        "task issue-7 is ready and did not stop on anything unresolved, so there is nothing to \
         resume; publish it with `sbxm task finish --issue 7`"
    );
}

#[test]
fn a_clean_ready_fix_task_resumes_to_its_push_but_takes_no_rounds() {
    let record = ready(Kind::Fix, None, 0);
    assert!(table(&record, Event::Resume));
    check_can_resume(&record, &Gone, None).unwrap();
    assert!(!table(&record, Event::ResumeRounds));
    let message = refusal(check_can_resume(&record, &Gone, Some(1)));
    assert!(
        message.contains("needs no more fix rounds; push it"),
        "{message}"
    );
}

#[test]
fn a_task_stopped_with_rounds_left_resumes_with_or_without_rounds() {
    for kind in [Kind::Issue, Kind::Spec, Kind::Fix] {
        let record = ready(kind, Some(Stopped::RepeatFinding), 1);
        assert!(table(&record, Event::Resume), "{kind:?}");
        check_can_resume(&record, &Gone, None).unwrap();
        assert!(table(&record, Event::ResumeRounds), "{kind:?}");
        check_can_resume(&record, &Gone, Some(1)).unwrap();
    }
}

#[test]
fn a_task_stopped_at_its_round_cap_needs_rounds() {
    for stopped in [Stopped::RepeatFinding, Stopped::RoundsExhausted] {
        let record = ready(Kind::Issue, Some(stopped), 2);
        assert!(!table(&record, Event::Resume), "{stopped:?}");
        assert_eq!(
            refusal(check_can_resume(&record, &Gone, None)),
            format!(
                "task issue-7 stopped ready ({}) with all 2 of its fix rounds used; give it more \
                 with `sbxm task resume --issue 7 --rounds N`",
                stopped.name()
            )
        );
        assert!(table(&record, Event::ResumeRounds), "{stopped:?}");
        check_can_resume(&record, &Gone, Some(1)).unwrap();
    }
}

#[test]
fn should_fix_is_for_a_clean_ready_task_in_the_table_and_the_check() {
    for kind in [Kind::Issue, Kind::Spec, Kind::Fix] {
        let clean = ready(kind, None, 0);
        assert!(table(&clean, Event::ResumeShouldFix), "{kind:?}");
        check_can_resume_with(&clean, &Gone, Some(1), true).unwrap();
    }
    for (stopped, round) in [
        (Stopped::RepeatFinding, 1),
        (Stopped::RepeatFinding, 2),
        (Stopped::RoundsExhausted, 2),
    ] {
        let mut record = ready(Kind::Issue, Some(stopped), round);
        record.review = None;
        assert!(!table(&record, Event::ResumeShouldFix), "{stopped:?}");
        // The message `check_should_fix` gives a stopped task.
        assert_eq!(
            refusal(check_can_resume_with(&record, &Gone, Some(1), true)),
            format!(
                "--should-fix is for a clean task (0 must-fix, no stop); task issue-7 stopped \
                 ready ({}) with 0 must-fix finding(s), so resume it without --should-fix",
                stopped.name()
            )
        );
    }
}

#[test]
fn task_states_lists_the_stop_rules() {
    let text = task_states::render();
    let has = |from: &str, event: &str| {
        assert!(
            text.lines()
                .any(|l| l.starts_with(&format!("| {from} | {event} |"))),
            "no row `{from} | {event}` in:\n{text}"
        );
    };
    has(
        "ready ok (issue/spec/fix), no live process, stopped with rounds left",
        "task resume",
    );
    has(
        "ready ok (fix), no live process, not stopped",
        "task resume",
    );
    has(
        "ready ok (issue/spec/fix), no live process, stopped",
        "task resume --rounds N",
    );
    has(
        "ready ok (issue/spec/fix), no live process, not stopped, no must-fix, should-fix round \
         unused",
        "task resume --rounds 1 --should-fix",
    );
}
