//! Issue 202, decision 195: archiving a merged task's text records before removal, and `remove`
//! (confirm once, archive, delete, report the disk freed).

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::Path;

use common::dir_link;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, ready_spec_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_finish;
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

/// Issue 202's M-2: a second archive of the same task id (a spec task re-run from the same file
/// gets the same deterministic id) must not touch or mix into the first archive.
#[test]
fn archiving_refuses_when_the_archive_directory_already_exists() {
    let f = fixture();
    finished_task(&f);
    let archive = archive_task(&f.env.base_dir(), "issue-41").unwrap();
    let first_result = fs::read_to_string(archive.join("result.md")).unwrap();
    // The live task folder changes before the (would-be) second run: a different result.md, and
    // a gates.log that didn't exist the first time.
    fs::write(meta(&f).join("result.md"), "a different result\n").unwrap();
    fs::write(meta(&f).join("gates.log"), "gate output\n").unwrap();

    let err = archive_task(&f.env.base_dir(), "issue-41").unwrap_err();

    assert!(
        format!("{err:#}").contains(&archive.display().to_string()),
        "{err:#}"
    );
    assert_eq!(
        fs::read_to_string(archive.join("result.md")).unwrap(),
        first_result,
        "the first archive's result.md was overwritten"
    );
    assert!(
        !archive.join("gates.log").exists(),
        "a file absent from the first run leaked into the archive"
    );
}

/// Issue 202's M-2: an archive destination that is a symlink or junction is refused without
/// following it, so nothing is ever written through it.
#[test]
fn archiving_refuses_a_symlinked_archive_destination() {
    let f = fixture();
    finished_task(&f);
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    let elsewhere = f.env.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    dir_link(&archive, &elsewhere);

    let err = archive_task(&f.env.base_dir(), "issue-41").unwrap_err();

    assert!(
        format!("{err:#}").contains(&archive.display().to_string()),
        "{err:#}"
    );
    assert!(
        fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "something was written through the symlink"
    );
    assert!(
        archive.symlink_metadata().unwrap().file_type().is_symlink(),
        "the symlink was replaced"
    );
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

/// A spec task finished through the `local` sink (decision 174(e)), then fetched into a fresh
/// checkout and merged into `main` there, as a real GitHub merge would land it: a candidate for
/// `task rm --merged`. Mirrors `tests/task_merged.rs`'s identically named helpers.
fn merged_spec_task(f: &Fixture) -> (String, std::path::PathBuf) {
    let (spec, id) = ready_spec_task(f);
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(spec),
            repo_root: repo_root(f),
            push_unresolved: false,
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    let checkout = f.env.tmp.path().join("checkout");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    common::git(
        &checkout,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    let scratch = f.env.tmp.path().join("merge-scratch");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    common::git(
        &scratch,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    common::git(&scratch, &["merge", "--ff-only", "-q", &id]);
    common::git(&scratch, &["push", "-q", "origin", "main"]);
    (id, checkout)
}

/// Issue 202's M-1: the post-removal summary, not only the dry-run listing, names the two git
/// commands that clean up a merged spec task's worktree and local branch in the user's own
/// checkout, which `task rm --merged` never touches.
#[test]
fn removing_a_merged_spec_task_names_its_worktree_and_branch_cleanup_commands() {
    let f = fixture();
    let (id, checkout) = merged_spec_task(&f);
    let github = FakeGitHub::default();
    let found = plan(&f.env.base_dir(), &checkout, &github, &Probe).unwrap();
    assert_eq!(found.candidates.len(), 1, "{found:?}");
    let confirm = Yes::new();
    let mut out = Vec::new();

    remove(
        &f.env.base_dir(),
        &found,
        true,
        &FakeBackend::with_secrets(&["anthropic", "openai"]),
        &confirm,
        rising_free_space,
        &mut out,
    )
    .unwrap();

    let printed = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = printed.lines().map(str::trim).collect();
    assert!(lines.contains(&"git worktree remove <path>"), "{printed}");
    assert!(
        lines.contains(&format!("git branch -d {id}").as_str()),
        "{printed}"
    );
    assert!(!printed.contains("branch -D"), "{printed}");
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

/// Moves the task folder `meta` elsewhere and leaves a link in its place; returns where it went.
fn link_task_folder(f: &Fixture, meta: &Path) -> std::path::PathBuf {
    let elsewhere = f.env.tmp.path().join("elsewhere-task");
    fs::rename(meta, &elsewhere).unwrap();
    dir_link(meta, &elsewhere);
    elsewhere
}

/// Issue 202's round-3 M-2: a task folder that is a link is refused before anything is read
/// through it or any archive is made.
#[test]
fn archiving_refuses_a_linked_task_folder_and_writes_nothing() {
    let f = fixture();
    finished_task(&f);
    let elsewhere = link_task_folder(&f, &meta(&f));
    let before: Vec<_> = fs::read_dir(&elsewhere)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();

    let err = archive_task(&f.env.base_dir(), "issue-41").unwrap_err();

    assert!(
        format!("{err:#}").contains(&meta(&f).display().to_string()),
        "{err:#}"
    );
    assert!(!archive_dir(&f.env.base_dir(), "issue-41").exists());
    let after: Vec<_> = fs::read_dir(&elsewhere)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(
        meta(&f)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// Issue 202's round-3 M-2: a linked `.sbxm/task-archive` is refused before any folder or file is
/// created through it.
#[test]
fn archiving_refuses_a_linked_archive_root() {
    let f = fixture();
    finished_task(&f);
    let root = archive_dir(&f.env.base_dir(), "issue-41")
        .parent()
        .unwrap()
        .to_path_buf();
    let elsewhere = f.env.tmp.path().join("elsewhere-archive");
    fs::create_dir_all(&elsewhere).unwrap();
    dir_link(&root, &elsewhere);

    let err = archive_task(&f.env.base_dir(), "issue-41").unwrap_err();

    assert!(
        format!("{err:#}").contains(&root.display().to_string()),
        "{err:#}"
    );
    assert!(
        fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "something was created through the link"
    );
}

/// Issue 202's round-3 M-2: a record inside the task folder that is a link isn't followed: what it
/// points at is never copied into the archive.
#[cfg(unix)]
#[test]
fn archiving_skips_a_record_that_is_a_link() {
    let f = fixture();
    finished_task(&f);
    let outside = f.env.tmp.path().join("outside.md");
    fs::write(&outside, "not the task's\n").unwrap();
    fs::remove_file(meta(&f).join("result.md")).unwrap();
    std::os::unix::fs::symlink(&outside, meta(&f).join("result.md")).unwrap();

    let archive = archive_task(&f.env.base_dir(), "issue-41").unwrap();

    assert!(archive.join("task.json").is_file());
    assert!(archive.join("result.md").symlink_metadata().is_err());
}

/// Issue 202's round-3 M-2: removing refuses a linked task folder before archiving, so the task
/// stays, nothing is archived, and nothing behind the link changes.
#[test]
fn removing_a_task_whose_folder_is_a_link_archives_and_deletes_nothing() {
    let f = fixture();
    let url = finished_task(&f);
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    let found = found_merged(&f, number);
    let elsewhere = link_task_folder(&f, &meta(&f));
    let mut out = Vec::new();

    let err = remove(
        &f.env.base_dir(),
        &found,
        true,
        &FakeBackend::with_secrets(&["anthropic"]),
        &Yes::new(),
        rising_free_space,
        &mut out,
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("stayed"), "{err:#}");
    assert!(!archive_dir(&f.env.base_dir(), "issue-41").exists());
    assert!(elsewhere.join("task.json").is_file());
    assert!(meta(&f).symlink_metadata().is_ok());
}
