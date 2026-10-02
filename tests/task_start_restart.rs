//! M2b slice 10, step 7: `sbxm task start --restart` discards the existing task (same
//! confirmation and removal as `task rm`) and starts it again (spec §4, decision 153).

mod common;

use std::cell::RefCell;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, issue_text, ok, open_issue, play_tasks,
    source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, Options, Restart};
use sbxm::confirm::Confirm;
use sbxm::github::IssueText;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Status};
use sbxm::task::repo::Identity;

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

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
    }
}

fn playing(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
}

fn go(
    f: &Fixture,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_start::run_with(
        &f.env.config_dir(),
        opts,
        restart,
        backend,
        github,
        &Probe,
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

/// A task for issue 41 that has already worked once.
fn started(f: &Fixture) {
    let (result, _) = go(f, &options(f), None, &playing(f), &github());
    result.unwrap();
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

#[test]
fn restart_asks_discards_the_old_task_and_starts_a_new_one() {
    let f = fixture();
    started(&f);
    let first = std::fs::read_to_string(meta(&f).join("task.json")).unwrap();
    let confirm = FakeConfirm::new(true, true);
    let b = playing(&f);

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    result.unwrap();
    let prompts = confirm.prompts.borrow();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&meta(&f).display().to_string()),
        "{}",
        prompts[0]
    );
    assert_eq!(b.removes(), vec!["sbxm-task-issue-41-claude"]);
    assert!(
        out.contains("removed sandbox sbxm-task-issue-41-claude"),
        "{out}"
    );
    assert!(out.contains("issue-41: worker completed"), "{out}");
    assert_eq!(b.creates().len(), 1);
    let again = std::fs::read_to_string(meta(&f).join("task.json")).unwrap();
    assert_ne!(first, again, "a fresh record");
}

#[test]
fn declining_keeps_the_old_task_and_starts_nothing() {
    let f = fixture();
    started(&f);
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, false),
            yes: false,
        }),
        &b,
        &github(),
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("cancelled"));
    assert!(meta(&f).join("task.json").exists());
    assert!(b.creates().is_empty() && b.removes().is_empty());
}

#[test]
fn without_a_terminal_restart_needs_yes_and_yes_is_enough() {
    let f = fixture();
    started(&f);
    let confirm = FakeConfirm::new(false, false);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &playing(&f),
        &github(),
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("pass --yes"));

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: true,
        }),
        &playing(&f),
        &github(),
    );
    result.unwrap();
    assert!(confirm.prompts.borrow().is_empty());
}

#[test]
fn a_running_task_is_not_restarted() {
    let f = fixture();
    started(&f);
    let mut task = record::read(&meta(&f).join("task.json")).unwrap();
    task.status = Status::Running;
    record::write(&meta(&f), &task).unwrap();
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &github(),
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("is running"));
    assert!(b.creates().is_empty() && b.removes().is_empty());
    assert!(meta(&f).join("task.json").exists());
}

#[test]
fn a_closed_issue_is_not_restarted_because_that_would_only_delete_the_work() {
    let f = fixture();
    started(&f);
    let closed = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(IssueText {
            state: "CLOSED".into(),
            ..issue_text(41)
        })
        .with_open_issues(vec![]);
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &closed,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("issue #41 is closed") && message.contains("task rm"),
        "{message}"
    );
    assert!(meta(&f).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn a_bad_flag_is_found_before_anything_is_deleted() {
    let f = fixture();
    started(&f);
    let mut opts = options(&f);
    opts.time_limit = Some("soon".into());
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &opts,
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &github(),
    );

    assert!(result.is_err());
    assert!(meta(&f).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn restart_without_issues_is_refused_and_a_task_that_does_not_exist_just_starts() {
    let f = fixture();
    let confirm = FakeConfirm::new(true, true);
    let restart = Restart {
        confirm: &confirm,
        yes: false,
    };
    let mut none = options(&f);
    none.issues.clear();
    none.workers = Some(1);
    let (result, _) = go(&f, &none, Some(&restart), &playing(&f), &github());
    assert!(format!("{:#}", result.unwrap_err()).contains("--restart needs"));

    let (result, out) = go(&f, &options(&f), Some(&restart), &playing(&f), &github());

    result.unwrap();
    assert!(
        confirm.prompts.borrow().is_empty(),
        "nothing to discard, nothing to ask"
    );
    assert!(out.contains("issue-41: worker completed"), "{out}");
}

#[test]
fn without_restart_an_existing_task_still_refuses() {
    let f = fixture();
    started(&f);

    let (result, out) = go(&f, &options(&f), None, &playing(&f), &github());

    let text = format!("{:#}{out}", result.unwrap_err());
    assert!(
        text.contains("already has a task") || text.contains("--restart"),
        "{text}"
    );
    assert!(meta(&f).join("task.json").exists());
}
