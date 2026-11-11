//! Issue #569: `finish` refuses a continued task (fix, resolve, log-fix) that changes the
//! repository's CI only when a CI file at the branch's tip differs from the base branch's tip as
//! the task's `repo.git` has it. A resolve that merged main brings main's own workflow changes
//! along; those already run on main, so pushing them adds nothing. A worker's own edit, addition
//! or deletion still differs and is still refused (decision 127).

mod common;

use std::fs;
use std::path::Path;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, add_pr_head, backend, ctx, fixture, ok, play, source,
};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{IssueText, PrInfo, PrState};
use sbxm::task::finish::finish;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::record::{self, Process, Stage, Status};

const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

fn pr7() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
        base_moved: false,
    }
}

fn finding() -> IssueText {
    IssueText {
        number: 41,
        title: "Fix 41".into(),
        state: "OPEN".into(),
        text: "title:\tFix 41\nstate:\tOPEN\nlabels:\tmust-fix\n--\nPR: #7\n\ndo it\n".into(),
    }
}

/// A commit on the remote's `main` that writes each `(path, Some(text))` and deletes each
/// `(path, None)`.
fn commit_to_main(f: &Fixture, files: &[(&str, Option<&str>)]) {
    let scratch = f.env.tmp.path().join("main-scratch");
    let _ = fs::remove_dir_all(&scratch);
    git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    write_files(&scratch, files);
    git(&scratch, &["commit", "-q", "-m", "main's change"]);
    git(&scratch, &["push", "-q", "origin", "main"]);
}

fn write_files(dir: &Path, files: &[(&str, Option<&str>)]) {
    for (path, text) in files {
        let full = dir.join(path);
        match text {
            Some(text) => {
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, text).unwrap();
            }
            None => fs::remove_file(&full).unwrap(),
        }
    }
    git(dir, &["add", "-A"]);
}

/// Issue 41 continues PR 7 (branch `feature-x` at the PR's head, made from the remote's `main`
/// as it is now); `before_task` then runs, the task is prepared (its `repo.git` cloned) and the
/// worker commits `b.txt`. The task is made `ready` by hand. Returns the task.
fn ready_after(f: &Fixture, before_task: impl FnOnce(&Fixture)) -> Prepared {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    before_task(f);
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "feature-x",
            Play {
                commits: vec!["b.txt".to_owned()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let github = FakeGitHub::default().with_pr(pr7());
    let source = source(f);
    let ctx = ctx(f, &source, &b, &github);
    let mut prepared = pipeline::prepare(&ctx, &finding()).unwrap();
    pipeline::run_worker(&ctx, &mut prepared).unwrap();
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    prepared.lock = None;
    prepared
}

/// What a resolve does in the task's `repo.git`: `feature-x` merges the `main` the task has
/// (`merge_main`), then a commit of the worker's own writes `files` on top (when any).
fn work_on_branch(prepared: &Prepared, merge_main: bool, files: &[(&str, Option<&str>)]) {
    let repo_git = prepared.meta.join("repo.git");
    let scratch = prepared.meta.join("branch-scratch");
    git(
        &prepared.meta,
        &[
            "clone",
            "-q",
            "-b",
            "feature-x",
            repo_git.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    if merge_main {
        git(
            &scratch,
            &[
                "merge",
                "-q",
                "--no-edit",
                "origin/main",
                "-m",
                "merge main",
            ],
        );
    }
    if !files.is_empty() {
        write_files(&scratch, files);
        git(&scratch, &["commit", "-q", "-m", "the worker's change"]);
    }
    git(&scratch, &["push", "-q", "origin", "feature-x"]);
    fs::remove_dir_all(&scratch).unwrap();
}

fn run(f: &Fixture) -> anyhow::Result<sbxm::task::finish::Finished> {
    finish(
        &f.env.base_dir(),
        "issue-41",
        &FakeGitHub::default(),
        &Probe,
    )
}

fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}

fn branch_tip(prepared: &Prepared) -> String {
    git(
        &prepared.meta.join("repo.git"),
        &["rev-parse", "refs/heads/feature-x"],
    )
}

const CI: &str = ".github/workflows/rust.yml";

#[test]
fn a_resolve_that_merged_mains_workflow_change_is_pushed() {
    let f = fixture();
    let prepared = ready_after(&f, |f| commit_to_main(f, &[(CI, Some("on: push\n"))]));
    work_on_branch(&prepared, true, &[]);

    run(&f).unwrap();

    assert_eq!(origin_head(&f), branch_tip(&prepared));
}

#[test]
fn a_resolve_that_merged_main_and_edited_its_workflow_is_refused() {
    let f = fixture();
    let prepared = ready_after(&f, |f| commit_to_main(f, &[(CI, Some("on: push\n"))]));
    let head = origin_head(&f);
    work_on_branch(&prepared, true, &[(CI, Some("on: push\nrun: evil\n"))]);

    let err = run(&f).unwrap_err();

    assert!(format!("{err:#}").contains(CI), "{err:#}");
    assert_eq!(origin_head(&f), head);
}

#[test]
fn a_workflow_main_does_not_have_is_refused() {
    let f = fixture();
    let prepared = ready_after(&f, |f| commit_to_main(f, &[(CI, Some("on: push\n"))]));
    let head = origin_head(&f);
    let new = ".github/workflows/new.yml";
    work_on_branch(&prepared, true, &[(new, Some("on: push\n"))]);

    let err = run(&f).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains(new), "{message}");
    assert!(!message.contains(CI), "{message}");
    assert_eq!(origin_head(&f), head);
}

#[test]
fn deleting_a_workflow_main_still_has_is_refused() {
    let f = fixture();
    // Main had the workflow before the PR was made, so the PR's head has it too.
    commit_to_main(&f, &[(CI, Some("on: push\n"))]);
    let prepared = ready_after(&f, |_| {});
    let head = origin_head(&f);
    work_on_branch(&prepared, false, &[(CI, None)]);

    let err = run(&f).unwrap_err();

    assert!(format!("{err:#}").contains(CI), "{err:#}");
    assert_eq!(origin_head(&f), head);
}

const ACTION: &str = ".github/actions/setup/action.yml";
// Mixed case below `.github` only: one checkout holds both files, and on a case-insensitive
// filesystem (Windows) a `.GitHub` folder next to `.github` is the same folder, so git would
// record the file under `.github` and the refusal would name that path.
const MIXED: &str = ".github/Workflows/other.yml";

#[test]
fn an_action_and_a_mixed_case_workflow_identical_to_mains_are_pushed() {
    let f = fixture();
    let prepared = ready_after(&f, |f| {
        commit_to_main(
            f,
            &[(ACTION, Some("runs: x\n")), (MIXED, Some("on: push\n"))],
        )
    });
    work_on_branch(&prepared, true, &[]);

    run(&f).unwrap();

    assert_eq!(origin_head(&f), branch_tip(&prepared));
}

#[test]
fn an_action_and_a_mixed_case_workflow_that_differ_from_mains_are_refused() {
    let f = fixture();
    let prepared = ready_after(&f, |f| {
        commit_to_main(
            f,
            &[(ACTION, Some("runs: x\n")), (MIXED, Some("on: push\n"))],
        )
    });
    let head = origin_head(&f);
    work_on_branch(
        &prepared,
        true,
        &[(ACTION, Some("runs: y\n")), (MIXED, Some("on: pull\n"))],
    );

    let err = run(&f).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains(ACTION), "{message}");
    assert!(message.contains(MIXED), "{message}");
    assert_eq!(origin_head(&f), head);
}

#[test]
fn a_resolve_that_merged_main_without_ci_changes_is_pushed_as_before() {
    let f = fixture();
    let prepared = ready_after(&f, |f| commit_to_main(f, &[("src.txt", Some("main\n"))]));
    work_on_branch(&prepared, true, &[("c.txt", Some("mine\n"))]);

    run(&f).unwrap();

    assert_eq!(origin_head(&f), branch_tip(&prepared));
}
