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

/// `task file-findings` takes no `--spec`: a spec task with must-fix findings left is finished,
/// as `task review --spec` and `task resume --spec` say.
#[test]
fn a_ready_spec_task_with_must_fix_left_names_a_command_that_exists() {
    assert_eq!(
        next::ready(&ready(Kind::Spec, 0, 2), "--spec plan.md").as_deref(),
        Some("sbxm task finish --spec plan.md")
    );
}

#[test]
fn a_ready_pr_review_files_its_must_fix_and_a_clean_one_has_nothing_to_run() {
    assert_eq!(
        next::ready(&ready(Kind::Pr, 8, 1), "--pr 8").as_deref(),
        Some("fix them by hand, or file them: sbxm task file-findings --pr 8")
    );
    assert_eq!(next::ready(&ready(Kind::Pr, 8, 0), "--pr 8"), None);
}

#[test]
fn a_ready_fix_task_is_pushed_by_its_resume() {
    for must_fix in [0, 1] {
        assert_eq!(
            next::ready(&ready(Kind::Fix, 9, must_fix), "--pr 9").as_deref(),
            Some("sbxm task resume --pr 9")
        );
    }
}
