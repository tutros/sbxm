//! M2b slice 10, step 3: `sbxm task rm` as a command: exact paths shown, confirmation,
//! `--yes`, refusals (spec §4, §12, decision 153).

mod common;

use std::cell::RefCell;
use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_rm::{Options, run};
use sbxm::confirm::Confirm;
use sbxm::task::record::{self, Kind, Status};

/// Answers from a script and records the prompts.
struct FakeConfirm {
    interactive: bool,
    answer: bool,
    prompts: RefCell<Vec<String>>,
}

impl FakeConfirm {
    fn new(interactive: bool, answer: bool) -> Self {
        Self {
            interactive,
            answer,
            prompts: RefCell::new(Vec::new()),
        }
    }
}

impl Confirm for FakeConfirm {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.prompts.borrow_mut().push(prompt.to_owned());
        Ok(self.answer)
    }
}

fn worked(f: &Fixture) -> FakeBackend {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    worked_task(f, &b);
    b
}

fn rm(
    f: &Fixture,
    number: u32,
    yes: bool,
    backend: &FakeBackend,
    confirm: &FakeConfirm,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            kind: Kind::Issue,
            number,
            yes,
        },
        backend,
        &Probe,
        confirm,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

#[test]
fn it_shows_the_exact_paths_and_sandbox_asks_and_then_deletes() {
    let f = fixture();
    let b = worked(&f);
    let confirm = FakeConfirm::new(true, true);

    let (result, out) = rm(&f, 41, false, &b, &confirm);

    result.unwrap();
    let prompts = confirm.prompts.borrow();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&meta(&f).display().to_string()),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("sandbox sbxm-task-issue-41-claude"),
        "{}",
        prompts[0]
    );
    assert!(!meta(&f).exists());
    assert!(
        out.contains("removed sandbox sbxm-task-issue-41-claude"),
        "{out}"
    );
}

#[test]
fn declining_deletes_nothing() {
    let f = fixture();
    let b = worked(&f);
    let removes_before = b.removes().len();

    let (result, _) = rm(&f, 41, false, &b, &FakeConfirm::new(true, false));

    assert!(format!("{:#}", result.unwrap_err()).contains("cancelled"));
    assert!(meta(&f).join("task.json").exists());
    assert!(f.env.base_dir().join("tasks").join("issue-41").exists());
    assert_eq!(b.removes().len(), removes_before);
}

#[test]
fn without_a_terminal_it_needs_yes_and_lists_what_it_would_delete() {
    let f = fixture();
    let b = worked(&f);
    let confirm = FakeConfirm::new(false, true);

    let (result, _) = rm(&f, 41, false, &b, &confirm);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("pass --yes") && message.contains("sbxm-task-issue-41-claude"),
        "{message}"
    );
    assert!(confirm.prompts.borrow().is_empty());
    assert!(meta(&f).join("task.json").exists());
}

#[test]
fn yes_skips_the_question_even_without_a_terminal() {
    let f = fixture();
    let b = worked(&f);
    let confirm = FakeConfirm::new(false, false);

    let (result, _) = rm(&f, 41, true, &b, &confirm);

    result.unwrap();
    assert!(confirm.prompts.borrow().is_empty());
    assert!(!meta(&f).exists());
}

#[test]
fn a_running_task_is_refused_and_nothing_is_asked_or_deleted() {
    let f = fixture();
    let b = backend();
    let mut prepared = worked_task(&f, &b);
    prepared.record.status = Status::Running;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let confirm = FakeConfirm::new(true, true);

    let (result, _) = rm(&f, 41, true, &b, &confirm);

    assert!(format!("{:#}", result.unwrap_err()).contains("is running"));
    assert!(confirm.prompts.borrow().is_empty());
    assert!(prepared.meta.join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn a_spec_task_is_refused_and_no_backend_call_or_question_happens() {
    let f = fixture();
    let b = backend();
    let confirm = FakeConfirm::new(true, true);
    let mut out = Vec::new();

    let result = run(
        &f.env.config_dir(),
        &Options {
            kind: Kind::Spec,
            number: 0,
            yes: true,
        },
        &b,
        &Probe,
        &confirm,
        &mut out,
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("doesn't take a spec task yet"));
    assert!(confirm.prompts.borrow().is_empty());
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(out.is_empty());
}

#[test]
fn an_unknown_task_says_so_and_a_failed_removal_fails_the_command() {
    let f = fixture();
    let (result, _) = rm(&f, 41, true, &backend(), &FakeConfirm::new(true, true));
    assert!(format!("{:#}", result.unwrap_err()).contains("no task issue-41"));

    let b = worked(&f);
    let stuck = FakeBackend::failing_remove().and_sandboxes(vec![sbxm::backend::SandboxInfo {
        name: "sbxm-task-issue-41-claude".into(),
        agent: "claude".into(),
        status: "running".into(),
    }]);
    drop(b);
    let (result, out) = rm(&f, 41, true, &stuck, &FakeConfirm::new(true, true));
    assert!(result.is_err());
    assert!(
        out.contains("could not remove sandbox sbxm-task-issue-41-claude"),
        "{out}"
    );
    assert!(fs::metadata(meta(&f).join("task.json")).is_ok());
}

#[test]
fn the_run_log_goes_with_the_task() {
    let f = fixture();
    let b = worked(&f);
    let log = meta(&f).join("run.log");
    std::fs::write(&log, "# header\n").unwrap();
    assert!(log.exists());

    let (result, _) = rm(&f, 41, true, &b, &FakeConfirm::new(true, true));

    result.unwrap();
    assert!(!log.exists());
    assert!(!meta(&f).exists());
}
