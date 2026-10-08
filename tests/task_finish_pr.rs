//! Decision 169 (e), issue #84: `task finish` for a task that continues an open PR's branch
//! pushes to that branch (a fast-forward from the head the task started at, never a force),
//! opens no new PR, and comments on the PR with the commits it added and `Fixes #n`.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, add_pr_head, backend, ctx, fixture, ok, play, source,
};
use sbxm::commands::task_finish::{Options, Target, run as finish_cmd};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{IssueText, PrInfo, PrState};
use sbxm::task::finish::finish;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::record::{self, Process, Stage, Status};

const REVIEW: &str = "Reviewer: codex (default)\nMust-fix findings: 0\n\nNothing found.\n";

fn pr7() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
    }
}

/// Issue 41, a finding of PR 7.
fn finding() -> IssueText {
    IssueText {
        number: 41,
        title: "Fix 41".into(),
        state: "OPEN".into(),
        text: "title:\tFix 41\nstate:\tOPEN\nlabels:\tmust-fix\n--\nPR: #7\n\ndo it\n".into(),
    }
}

/// Issue 41 continues PR 7 (branch `feature-x` on the remote, at the PR's head): the worker
/// commits `files` on top, and the gates and review are done by hand, so the task is `ready`.
/// Returns the task and the PR head it started from.
fn ready_with(f: &Fixture, files: &[&str]) -> (Prepared, String) {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "feature-x",
            Play {
                commits: files.iter().map(|s| (*s).to_owned()).collect(),
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
    (prepared, head)
}

fn ready(f: &Fixture) -> (Prepared, String) {
    ready_with(f, &["b.txt"])
}

fn reread(f: &Fixture) -> record::Record {
    Prepared::open(&f.env.base_dir(), "issue-41")
        .unwrap()
        .record
}

fn run(f: &Fixture, github: &FakeGitHub) -> anyhow::Result<sbxm::task::finish::Finished> {
    finish(&f.env.base_dir(), "issue-41", github, &Probe)
}

fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}

/// Someone else pushes a commit to the PR's branch on the remote meanwhile.
fn push_from_elsewhere(f: &Fixture) {
    let scratch = f.env.tmp.path().join("elsewhere");
    git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            "-b",
            "feature-x",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    fs::write(scratch.join("other.txt"), "someone else\n").unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "someone else's change"]);
    git(&scratch, &["push", "-q", "origin", "feature-x"]);
}

#[test]
fn a_continued_task_is_pushed_to_the_pr_branch_with_a_comment_and_no_new_pr() {
    let f = fixture();
    let (prepared, head) = ready(&f);
    let repo_git = prepared.meta.join("repo.git");
    let tip = git(&repo_git, &["rev-parse", "refs/heads/feature-x"]);
    assert_ne!(tip, head);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    // The remote's PR branch is now the task's branch: a fast-forward from the PR head.
    assert_eq!(origin_head(&f), tip);
    assert_eq!(
        git(
            &f.origin,
            &["rev-list", "--count", &format!("{head}..{tip}")]
        ),
        "1"
    );
    // No issue-41 branch went anywhere.
    assert!(git(&f.origin, &["branch", "--list", "issue-41"]).is_empty());
    let calls = github.calls();
    let [GhCall::PrComment(repo, 7, body)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert_eq!(repo, "o/r");
    assert!(body.contains("Fixes #41"), "{body}");
    assert!(body.contains(&tip[..7]), "{body}");
    assert!(body.contains("b.txt"), "{body}");
    assert!(!body.contains(&head[..7]), "{body}");
    assert_eq!(done.url, "https://github.com/o/r/pull/7");
    assert_eq!(done.continued, Some(7));
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr.as_deref(), Some("https://github.com/o/r/pull/7"));
}

#[test]
fn a_pr_branch_that_moved_on_the_remote_is_refused_and_nothing_is_pushed() {
    let f = fixture();
    let (prepared, _) = ready(&f);
    push_from_elsewhere(&f);
    let theirs = origin_head(&f);
    let before = fs::read_to_string(prepared.meta.join("task.json")).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("feature-x"), "{message}");
    assert!(message.contains("moved"), "{message}");
    assert!(message.contains("never forces"), "{message}");
    assert_eq!(origin_head(&f), theirs);
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert_eq!(
        fs::read_to_string(prepared.meta.join("task.json")).unwrap(),
        before
    );
}

#[test]
fn a_pr_branch_moved_back_on_the_remote_is_refused_too() {
    let f = fixture();
    let (_, _) = ready(&f);
    let main = git(&f.origin, &["rev-parse", "refs/heads/main"]);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &main]);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("moved"), "{err:#}");
    assert_eq!(origin_head(&f), main);
    assert!(github.calls().is_empty());
}

#[test]
fn a_pr_branch_gone_from_the_remote_is_refused() {
    let f = fixture();
    let (_, _) = ready(&f);
    git(&f.origin, &["update-ref", "-d", "refs/heads/feature-x"]);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("isn't on the remote"), "{message}");
    assert!(git(&f.origin, &["branch", "--list", "feature-x"]).is_empty());
    assert!(github.calls().is_empty());
}

#[test]
fn a_comment_that_fails_after_the_push_leaves_the_task_ready_and_a_rerun_only_comments() {
    let f = fixture();
    let (prepared, _) = ready(&f);
    let tip = git(
        &prepared.meta.join("repo.git"),
        &["rev-parse", "refs/heads/feature-x"],
    );
    let down = FakeGitHub::default().failing_comments("gh: HTTP 502");

    let err = run(&f, &down).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("pushed feature-x but could not comment on PR #7"),
        "{message}"
    );
    assert!(message.contains("gh: HTTP 502"), "{message}");
    assert_eq!(origin_head(&f), tip);
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(record.pr.is_none());
    assert_eq!(record.notes.len(), 1, "{:?}", record.notes);

    // The remote is now at the task's own tip, which a rerun accepts without pushing again: a
    // push would now fail.
    git(
        &prepared.meta.join("repo.git"),
        &["config", "remote.origin.receivepack", "false"],
    );
    let up = FakeGitHub::default();
    run(&f, &up).unwrap();

    assert_eq!(origin_head(&f), tip);
    assert!(
        matches!(up.calls().as_slice(), [GhCall::PrComment(_, 7, _)]),
        "{:?}",
        up.calls()
    );
    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert!(record.notes.is_empty(), "{:?}", record.notes);
}

#[test]
fn a_continued_task_without_commits_of_its_own_is_refused() {
    let f = fixture();
    let (prepared, head) = ready(&f);
    git(
        &prepared.meta.join("repo.git"),
        &["update-ref", "refs/heads/feature-x", &head],
    );
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("no commits"), "{err:#}");
    assert!(github.calls().is_empty());
    assert_eq!(origin_head(&f), head);
}

#[test]
fn a_workflow_change_on_top_of_the_pr_is_refused() {
    let f = fixture();
    let (_, head) = ready_with(&f, &[".github/workflows/ci.yml"]);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(
        format!("{err:#}").contains(".github/workflows/ci.yml"),
        "{err:#}"
    );
    assert!(github.calls().is_empty());
    assert_eq!(origin_head(&f), head);
}

#[test]
fn the_command_says_it_pushed_to_the_pr_and_how_to_review_it_again() {
    let f = fixture();
    ready(&f);
    let mut out = Vec::new();

    finish_cmd(
        &f.env.config_dir(),
        &Options {
            target: Target::Issue(41),
            repo_root: f.env.tmp.path().join("target-repo"),
        },
        &FakeGitHub::default(),
        &Probe,
        &mut out,
    )
    .unwrap();

    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(
            "issue-41: pushed feature-x to PR #7 and commented on it, https://github.com/o/r/pull/7"
        ),
        "{out}"
    );
    assert!(out.contains("sbxm task review --pr 7"), "{out}");
    assert!(!out.contains("opened"), "{out}");
}

#[test]
fn a_pr_branch_moved_between_the_check_and_the_push_is_refused_and_left_alone() {
    let f = fixture();
    let (prepared, head) = ready_with(&f, &["b.txt", "c.txt"]);
    let repo_git = prepared.meta.join("repo.git");
    let middle = git(&repo_git, &["rev-parse", "refs/heads/feature-x~1"]);
    assert_ne!(middle, head);
    git(
        &repo_git,
        &["push", "-q", "origin", &format!("{middle}:refs/heads/side")],
    );
    // `ls-remote` reads the remote through upload-pack; once it has answered (with the task's
    // start commit), the remote branch moves on to a commit between that start and the task's
    // tip, which a plain push would fast-forward over.
    git(
        &repo_git,
        &[
            "config",
            "remote.origin.uploadpack",
            &format!(
                "race() {{ git upload-pack \"$@\"; s=$?; git --git-dir='{}' update-ref refs/heads/feature-x {middle}; return $s; }}; race",
                f.origin.to_str().unwrap().replace(char::from(92), "/")
            ),
        ],
    );
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("moved"), "{err:#}");
    assert_eq!(origin_head(&f), middle);
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert_eq!(reread(&f).stage, Stage::Ready);
}

/// Gives the tip commit of `feature-x` in the task's repo the message `message`.
fn reword_tip(prepared: &Prepared, message: &str) -> String {
    let repo_git = prepared.meta.join("repo.git");
    let tip = git(
        &repo_git,
        &[
            "commit-tree",
            "refs/heads/feature-x^{tree}",
            "-p",
            "refs/heads/feature-x~1",
            "-m",
            message,
        ],
    );
    git(&repo_git, &["update-ref", "refs/heads/feature-x", &tip]);
    tip
}

fn posted_comment(github: &FakeGitHub) -> String {
    let calls = github.calls();
    let [GhCall::PrComment(_, 7, body)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    body.clone()
}

#[test]
fn a_commit_subject_longer_than_github_accepts_is_cut_and_the_task_finishes() {
    let f = fixture();
    let (prepared, _) = ready(&f);
    let tip = reword_tip(&prepared, &"y".repeat(70_000));
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let body = posted_comment(&github);
    assert!(body.chars().count() < 65_536, "{}", body.chars().count());
    assert!(body.contains(&tip[..7]), "{}", &body[..300]);
    assert!(body.contains("Fixes #41"), "{}", &body[body.len() - 300..]);
    assert!(body.contains("cut"), "{}", &body[body.len() - 300..]);
    assert_eq!(origin_head(&f), tip);
    assert_eq!(reread(&f).stage, Stage::Finished);
}

#[test]
fn a_comment_with_too_many_commits_lists_what_fits_and_says_how_many_more() {
    let commits: Vec<String> = (0..2_000)
        .map(|i| format!("{i:07x} {}", "z".repeat(200)))
        .collect();

    let body = sbxm::task::finish::pr_comment(41, "feature-x", &commits);

    assert!(body.chars().count() < 65_536, "{}", body.chars().count());
    assert!(body.contains("`0000000`"), "{}", &body[..300]);
    assert!(
        body.contains("more commit"),
        "{}",
        &body[body.len() - 300..]
    );
    assert!(body.contains("2000 commit(s)"), "{}", &body[..300]);
    assert!(body.contains("Fixes #41"), "{}", &body[body.len() - 300..]);
}

#[test]
fn mentions_in_commit_subjects_do_not_ping_anyone() {
    let f = fixture();
    let (prepared, _) = ready(&f);
    reword_tip(
        &prepared,
        "Ask @someone and @team/leads; mail a@b.com; (@x) too",
    );
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let body = posted_comment(&github);
    assert!(
        !body.contains("@someone") && !body.contains("@team") && !body.contains("(@x)"),
        "{body}"
    );
    assert!(
        body.contains("someone") && body.contains("a@b.com"),
        "{body}"
    );
}
