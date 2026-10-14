//! Issue 202, decision 195: `sbxm task rm --merged [--dry-run] [--yes]` as a command.

mod common;

use std::cell::RefCell;
use std::fs;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_rm::{Options, Target, run};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::finish::{finish, pr_number};
use sbxm::task::record::{self, Process, Stage, Status};

/// As `review::with_header` saves it: the reviewer's line, a blank line, then the review.
const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

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

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

/// Issue 41 worked, its gates and review done by hand, then finished (pushed, PR opened).
fn finished_task(f: &Fixture) -> u32 {
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
    let mut prepared = common::task_fixture::worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    let github = FakeGitHub::default();
    let url = finish(&f.env.base_dir(), "issue-41", &github, &Probe)
        .unwrap()
        .url;
    pr_number(&url).unwrap()
}

fn merged_github(number: u32) -> FakeGitHub {
    FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    })
}

fn rm_merged(
    f: &Fixture,
    dry_run: bool,
    yes: bool,
    backend: &FakeBackend,
    github: &FakeGitHub,
    confirm: &dyn Confirm,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Merged,
            yes,
            force: false,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run,
        },
        backend,
        github,
        &Probe,
        confirm,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn a_dry_run_lists_the_merged_task_and_removes_nothing() {
    let f = fixture();
    let number = finished_task(&f);
    let github = merged_github(number);
    let confirm = FakeConfirm::new(true, true);

    let (result, out) = rm_merged(&f, true, false, &backend(), &github, &confirm);

    result.unwrap();
    assert!(out.contains("issue-41"), "{out}");
    assert!(out.contains(&format!("#{number}")), "{out}");
    assert!(confirm.prompts.borrow().is_empty(), "dry run must not ask");
    assert!(meta(&f).exists(), "dry run must not remove anything");
}

#[test]
fn a_dry_run_with_nothing_merged_says_so() {
    let f = fixture();
    let github = FakeGitHub::default();
    let confirm = FakeConfirm::new(true, true);

    let (result, out) = rm_merged(&f, true, false, &backend(), &github, &confirm);

    result.unwrap();
    assert!(out.contains("no merged tasks"), "{out}");
}

#[test]
fn yes_removes_the_merged_task_without_asking() {
    let f = fixture();
    let number = finished_task(&f);
    let github = merged_github(number);
    let confirm = FakeConfirm::new(true, true);

    let (result, out) = rm_merged(&f, false, true, &backend(), &github, &confirm);

    result.unwrap();
    assert!(confirm.prompts.borrow().is_empty());
    assert!(!meta(&f).exists(), "{out}");
    let archive = sbxm::task::merged::archive_dir(&f.env.base_dir(), "issue-41").join("task.json");
    assert!(archive.is_file(), "{out}");
}

#[test]
fn without_yes_it_asks_once_for_every_merged_task() {
    let f = fixture();
    let number = finished_task(&f);
    let github = merged_github(number);
    let confirm = FakeConfirm::new(true, true);

    let (result, _out) = rm_merged(&f, false, false, &backend(), &github, &confirm);

    result.unwrap();
    assert_eq!(confirm.prompts.borrow().len(), 1);
    assert!(!meta(&f).exists());
}

#[test]
fn declining_the_confirmation_removes_nothing() {
    let f = fixture();
    let number = finished_task(&f);
    let github = merged_github(number);
    let confirm = FakeConfirm::new(true, false);

    let (result, _out) = rm_merged(&f, false, false, &backend(), &github, &confirm);

    assert!(result.is_err());
    assert!(meta(&f).exists());
}
