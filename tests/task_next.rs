//! Issue 512: one function gives a task's flag and the command that continues it, by its kind
//! (issue, PR review, fix, spec); each output pinned here.

use sbxm::task::next;
use sbxm::task::record::{Kind, NewTask, Process, Record, ReviewResult, Stage, Status};

fn task(kind: Kind, number: u32) -> Record {
    let id = (kind == Kind::Spec).then_some("spec-plan-0123456789ab");
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "sbxm/t",
            config_hash: "h",
            id,
        },
        0,
        Process::new(1, 0),
    )
}

fn ready(kind: Kind, number: u32, must_fix: u32) -> Record {
    let mut rec = task(kind, number);
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.review = Some(ReviewResult {
        round: 1,
        must_fix,
        ..ReviewResult::default()
    });
    rec
}

#[test]
fn the_flag_names_each_kind_of_task() {
    assert_eq!(next::record_flag(&task(Kind::Issue, 7)), "--issue 7");
    assert_eq!(next::record_flag(&task(Kind::Pr, 8)), "--pr 8");
    assert_eq!(next::record_flag(&task(Kind::Fix, 9)), "--pr 9");
    assert_eq!(next::record_flag(&task(Kind::Spec, 0)), "--spec <file>");
}

#[test]
fn a_stopped_stage_goes_on_with_each_kinds_command() {
    assert_eq!(
        next::resume(&task(Kind::Issue, 7)),
        "sbxm task resume --issue 7"
    );
    assert_eq!(next::resume(&task(Kind::Pr, 8)), "sbxm task review --pr 8");
    assert_eq!(next::resume(&task(Kind::Fix, 9)), "sbxm task resume --pr 9");
    assert_eq!(
        next::resume(&task(Kind::Spec, 0)),
        "sbxm task resume --spec <file>"
    );
}

#[test]
fn a_clean_ready_issue_or_spec_task_is_finished() {
    assert_eq!(
        next::ready(&ready(Kind::Issue, 7, 0), "--issue 7").as_deref(),
        Some("sbxm task finish --issue 7")
    );
    assert_eq!(
        next::ready(&ready(Kind::Spec, 0, 0), "--spec plan.md").as_deref(),
        Some("sbxm task finish --spec plan.md")
    );
}

#[test]
fn a_ready_issue_task_with_must_fix_left_files_them() {
    assert_eq!(
        next::ready(&ready(Kind::Issue, 7, 2), "--issue 7").as_deref(),
        Some("fix them by hand, or file them: sbxm task file-findings --issue 7")
    );
}

#[test]
fn a_task_not_ready_has_no_ready_step() {
    assert_eq!(next::ready(&task(Kind::Issue, 7), "--issue 7"), None);
}
