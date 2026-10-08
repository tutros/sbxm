//! Issue 149 (review of PR 156, M-2): when a review is refused, the hint names a command that
//! works for the task at hand. A spec task has no issue number, so `--issue 0` would select
//! nothing; an issue task still names its real number.

use sbxm::task::pipeline::check_can_review;
use sbxm::task::record::{Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

const T0: u64 = 1_790_000_000;

fn task(kind: Kind) -> Record {
    let (number, id) = match kind {
        Kind::Spec => (0, Some("spec-idea-abc123")),
        _ => (7, None),
    };
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id,
        },
        T0,
        Process::new(1, T0),
    )
}

fn at(kind: Kind, stage: Stage, status: Status) -> Record {
    let mut record = task(kind);
    for step in [Stage::Working, Stage::Gating, Stage::Reviewing] {
        if record.stage == stage {
            break;
        }
        record.advance(step, T0, Process::new(1, T0)).unwrap();
        if step != stage {
            let done = if step == Stage::Working {
                Status::Completed
            } else {
                Status::Passed
            };
            record.finish(done).unwrap();
        }
    }
    record.status = status;
    record
}

fn refusal(record: &Record, running_process: bool) -> String {
    let probe = Probe(running_process.then_some(T0));
    format!("{:#}", check_can_review(record, &probe).unwrap_err())
}

#[test]
fn spec_task_refusals_never_name_issue_0() {
    let id = "spec-idea-abc123";
    let cases = [
        (
            "worker failed",
            at(Kind::Spec, Stage::Working, Status::Failed),
            true,
        ),
        (
            "interrupted",
            at(Kind::Spec, Stage::Working, Status::Running),
            false,
        ),
        (
            "gating",
            at(Kind::Spec, Stage::Gating, Status::Running),
            true,
        ),
        (
            "reviewing",
            at(Kind::Spec, Stage::Reviewing, Status::Running),
            true,
        ),
        ("prepared", task(Kind::Spec), true),
    ];
    for (name, record, alive) in cases {
        let message = refusal(&record, alive);
        assert!(message.contains(id), "{name}: {message}");
        assert!(!message.contains("--issue"), "{name}: {message}");
        assert!(message.contains("sbxm task status"), "{name}: {message}");
    }
}

#[test]
fn a_prepared_spec_task_is_not_told_to_start_again() {
    let message = refusal(&task(Kind::Spec), true);
    assert!(!message.contains("`sbxm task start` first"), "{message}");
}

#[test]
fn an_issue_task_refusal_still_names_its_issue() {
    let message = refusal(&at(Kind::Issue, Stage::Working, Status::Failed), true);
    assert!(message.contains("sbxm task status --issue 7"), "{message}");
}

/// Issue 118: a review refused because a stage failed or was cut off names `task resume`, which
/// continues it, instead of leaving only `task status` (and `rm`) to the user.
#[test]
fn a_failed_or_interrupted_stage_names_task_resume() {
    let cases = [
        ("worker failed", Stage::Working, Status::Failed, true),
        ("worker interrupted", Stage::Working, Status::Running, false),
        ("fix round failed", Stage::Fixing, Status::Failed, true),
        (
            "review interrupted",
            Stage::Reviewing,
            Status::Running,
            false,
        ),
        (
            "review completed",
            Stage::Reviewing,
            Status::Completed,
            false,
        ),
    ];
    // `check_can_review` reads only the stage, the status, the kind and the process.
    let left_in = |kind, stage, status| {
        let mut record = task(kind);
        (record.stage, record.status) = (stage, status);
        record
    };
    for (name, stage, status, alive) in cases {
        let message = refusal(&left_in(Kind::Issue, stage, status), alive);
        assert!(
            message.contains("sbxm task resume --issue 7"),
            "{name}: {message}"
        );
        let message = refusal(&left_in(Kind::Spec, stage, status), alive);
        assert!(
            message.contains("sbxm task resume --spec <file>"),
            "{name}: {message}"
        );
        assert!(!message.contains("--issue"), "{name}: {message}");
    }
}
