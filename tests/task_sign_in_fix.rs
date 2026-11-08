//! Issue 502: a fix task's lapsed harness sign-in names `task resume --pr N` (issue 485) as the
//! way on, not "no command continues one yet".

use sbxm::harness::Harness;
use sbxm::task::pipeline::harness_sign_in_lapsed;
use sbxm::task::record::{Kind, NewTask, Process, Record};

fn fix_task_of_pr_7() -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Fix,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: Some("fix-7-1234567890ab"),
        },
        0,
        Process::new(1, 0),
    )
}

#[test]
fn a_fix_tasks_lapsed_sign_in_names_task_resume_pr() {
    let message = harness_sign_in_lapsed(
        "the worker",
        Harness::Claude,
        "default",
        &fix_task_of_pr_7(),
        "OAuth session expired",
    );

    assert!(
        message.contains("continue with `sbxm task resume --pr 7`"),
        "{message}"
    );
    assert!(!message.contains("no command continues"), "{message}");
}
