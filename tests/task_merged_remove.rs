//! Issue 202, decision 195: archiving a merged task's text records before removal, and `remove`
//! (confirm once, archive, delete, report the disk freed).

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::Path;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play};
use sbxm::backend::FakeBackend;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::finish::finish;
use sbxm::task::merged::{archive_dir, archive_task, plan, remove};
use sbxm::task::record::{self, Process, Stage, Status};

/// As `review::with_header` saves it: the reviewer's line, a blank line, then the review.
const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

/// Issue 41 worked, its gates and review done by hand, then finished (pushed, PR opened).
fn finished_task(f: &Fixture) -> String {
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
    finish(&f.env.base_dir(), "issue-41", &github, &Probe)
        .unwrap()
        .url
}

fn repo_root(f: &Fixture) -> std::path::PathBuf {
    f.env.tmp.path().join("target-repo")
}

struct Yes(RefCell<Vec<String>>);

impl Yes {
    fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }

    fn prompts(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.0.borrow_mut().push(prompt.to_owned());
        Ok(true)
    }
}

struct NonInteractive;

impl Confirm for NonInteractive {
    fn is_interactive(&self) -> bool {
        false
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        unreachable!("never asked without a terminal")
    }
}

// Two different free-space readings, so `remove`'s "freed" report is deterministic regardless of
// the real disk: the first call (before removal) reads less free space than the second (after).
thread_local! {
    static CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn rising_free_space(_: &Path) -> std::io::Result<u64> {
    let n = CALLS.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    });
    Ok(if n == 0 { 1_000_000_000 } else { 1_600_000_000 })
}

fn unreadable_free_space(_: &Path) -> std::io::Result<u64> {
    Err(std::io::Error::other("no such drive"))
}

#[test]
fn archiving_copies_only_the_text_records_that_exist() {
    let f = fixture();
    finished_task(&f);
    let dir = meta(&f);
    fs::write(dir.join("review-1.md"), "round 1\n").unwrap();
    fs::write(dir.join("review-2.md"), "round 2\n").unwrap();
    fs::write(dir.join("gates.log"), "gate output\n").unwrap();
    fs::write(dir.join("run.log"), "run output\n").unwrap();
    // Never archived: a transcript and the clones.
    fs::write(dir.join("worker.transcript.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(dir.join("repo.git")).unwrap();

    let archive = archive_task(&f.env.base_dir(), "issue-41").unwrap();

    assert_eq!(archive, archive_dir(&f.env.base_dir(), "issue-41"));
    for name in [
        "review.md",
        "review-1.md",
        "review-2.md",
        "result.md",
        "run.log",
        "gates.log",
        "task.json",
    ] {
        assert!(archive.join(name).is_file(), "{name} missing from archive");
    }
    assert!(!archive.join("worker.transcript.jsonl").exists());
    assert!(!archive.join("repo.git").exists());
    assert_eq!(
        fs::read_to_string(archive.join("review-2.md")).unwrap(),
        "round 2\n"
    );
}

#[test]
fn archiving_a_task_with_no_optional_files_copies_what_it_has() {
    let f = fixture();
    finished_task(&f);
    // No review-<n>.md, no run.log, no gates.log in this fixture.

    let archive = archive_task(&f.env.base_dir(), "issue-41").unwrap();

    assert!(archive.join("review.md").is_file());
    assert!(archive.join("result.md").is_file());
    assert!(archive.join("task.json").is_file());
    assert!(!archive.join("run.log").exists());
    assert!(!archive.join("gates.log").exists());
}

fn found_merged(f: &Fixture, number: u32) -> sbxm::task::merged::MergedPlan {
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });
    plan(&f.env.base_dir(), &repo_root(f), &github, &Probe).unwrap()
}

#[test]
fn removing_archives_first_then_deletes_the_sandbox_and_folders_and_reports_freed_disk() {
    let f = fixture();
    let url = finished_task(&f);
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    let found = found_merged(&f, number);
    assert_eq!(found.candidates.len(), 1);
    let confirm = Yes::new();
    let mut out = Vec::new();

    remove(
        &f.env.base_dir(),
        &found,
        false,
        &FakeBackend::with_secrets(&["anthropic"]),
        &confirm,
        rising_free_space,
        &mut out,
    )
    .unwrap();

    assert_eq!(confirm.prompts().len(), 1);
    assert!(!meta(&f).exists(), "the task folder was not removed");
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert!(archive.join("task.json").is_file());
    assert!(archive.join("review.md").is_file());
    let printed = String::from_utf8(out).unwrap();
    assert!(printed.contains("archived task issue-41"), "{printed}");
    assert!(printed.contains("removed"), "{printed}");
    assert!(printed.contains("freed about"), "{printed}");
}

#[test]
fn removing_with_yes_asks_nothing() {
    let f = fixture();
    let url = finished_task(&f);
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    let found = found_merged(&f, number);
    let confirm = Yes::new();
    let mut out = Vec::new();

    remove(
        &f.env.base_dir(),
        &found,
        true,
        &FakeBackend::with_secrets(&["anthropic"]),
        &confirm,
        rising_free_space,
        &mut out,
    )
    .unwrap();

    assert!(confirm.prompts().is_empty());
    assert!(!meta(&f).exists());
}

#[test]
fn removing_without_yes_or_a_terminal_is_refused_and_nothing_is_touched() {
    let f = fixture();
    let url = finished_task(&f);
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    let found = found_merged(&f, number);
    let mut out = Vec::new();

    let err = remove(
        &f.env.base_dir(),
        &found,
        false,
        &FakeBackend::with_secrets(&["anthropic"]),
        &NonInteractive,
        rising_free_space,
        &mut out,
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("--yes"));
    assert!(meta(&f).exists(), "the task folder was removed anyway");
    assert!(!archive_dir(&f.env.base_dir(), "issue-41").exists());
}

#[test]
fn removing_nothing_merged_prints_a_message_and_asks_nothing() {
    let f = fixture();
    let found = sbxm::task::merged::MergedPlan::default();
    let confirm = Yes::new();
    let mut out = Vec::new();

    remove(
        &f.env.base_dir(),
        &found,
        false,
        &FakeBackend::with_secrets(&["anthropic"]),
        &confirm,
        rising_free_space,
        &mut out,
    )
    .unwrap();

    assert!(confirm.prompts().is_empty());
    let printed = String::from_utf8(out).unwrap();
    assert!(printed.contains("no merged tasks"), "{printed}");
}

#[test]
fn removing_never_fails_when_the_disk_cannot_be_read() {
    let f = fixture();
    let url = finished_task(&f);
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    let found = found_merged(&f, number);
    let confirm = Yes::new();
    let mut out = Vec::new();

    remove(
        &f.env.base_dir(),
        &found,
        true,
        &FakeBackend::with_secrets(&["anthropic"]),
        &confirm,
        unreadable_free_space,
        &mut out,
    )
    .unwrap();

    let printed = String::from_utf8(out).unwrap();
    assert!(!printed.contains("freed"), "{printed}");
}
