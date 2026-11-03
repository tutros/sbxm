//! Issue 313: a fix task of a PR, through the pipeline. It starts on the PR's own branch at the
//! head its review covered, its worker gets that review's must-fix findings only, the gates and
//! the in-task review follow as for any task, and `finish_fix` pushes the result as a
//! fast-forward and posts the review as the new head's, or pushes nothing when the branch moved.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, ctx, fixture, ok, play,
    play_reviews, source,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::finish::finish_fix;
use sbxm::task::merged;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::pr_fix::{ReviewToFix, review_to_fix};
use sbxm::task::record::{self, Kind, Stage, Status};
use sbxm::task::review::pr_comment;

fn pr7(head: &str) -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
    }
}

/// The PR review the user posted, covering `head`, with one must-fix and one should-fix finding.
fn posted_review(head: &str) -> PrComment {
    let review = format!(
        "Must-fix findings: 1\nRisk: medium\n\nScope: `0000000..{head}`\nIssues: pending\n\n\
         ## Must fix\n\n### M-1 - The lock is never released\n\n**Where:** `src/lock.rs:10`\n\
         **What happens:** it stays held\n**Why it matters:** decision 1\n**Fix:** release it\n\n\
         ## Should fix\n\n### S-1 - A name is unclear\n\n**Where:** `src/name.rs:1`\n\
         **What happens:** x\n**Why it matters:** y\n**Fix:** rename it\n"
    );
    PrComment {
        author: "tutros".into(),
        body: pr_comment(7, "", &review),
    }
}

/// PR 7's head on the remote (branch `feature-x`), and the review to fix it from.
fn pr_with_review(f: &Fixture) -> (PrInfo, ReviewToFix) {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    let pr = pr7(&head);
    let review = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(&head)]).unwrap();
    (pr, review)
}

/// The in-task reviewer's text: clean, its Scope ending at the commit it reviewed.
fn clean_review() -> String {
    "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing found.\n".to_owned()
}

fn backend_for(f: &Fixture, id: &str, reviews: Vec<String>) -> FakeBackend {
    let worker = play(
        &f.env.base_dir().join("tasks").join(id),
        "main",
        "feature-x",
        Play {
            commits: vec!["src/lock.rs".into()],
            result_md: Some(b"M-1 fixed\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), id, reviews);
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        })
}

/// Prepares the fix task of PR 7, runs its worker and its review: it ends `ready`.
fn ready_fix(f: &Fixture) -> (Prepared, String) {
    let (pr, review) = pr_with_review(f);
    let id = record::fix_id(7, &pr.head_oid).unwrap();
    let backend = backend_for(f, &id, vec![clean_review()]);
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    pipeline::review_issue(&context.env(), &mut prepared).unwrap();
    assert_eq!(
        (prepared.record.stage, prepared.record.status),
        (Stage::Ready, Status::Ok)
    );
    // The start that made it has ended: its lock (issue 58) goes with it.
    prepared.lock = None;
    (prepared, pr.head_oid)
}

fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}

#[test]
fn a_fix_task_starts_on_the_pr_branch_at_the_reviewed_head_with_the_must_fix_findings_only() {
    let f = fixture();
    let (pr, review) = pr_with_review(&f);
    let id = record::fix_id(7, &pr.head_oid).unwrap();
    let backend = backend_for(&f, &id, vec![clean_review()]);
    let github = FakeGitHub::default();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);

    let prepared = pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap();

    let task = &prepared.record;
    assert_eq!(task.id, id);
    assert_eq!((task.kind, task.number), (Kind::Fix, 7));
    assert_eq!(task.branch, "feature-x");
    let continues = task.continues.as_ref().unwrap();
    assert_eq!(
        (continues.pr, continues.base.as_str()),
        (7, pr.head_oid.as_str())
    );
    let input = fs::read_to_string(prepared.meta.join("issue.md")).unwrap();
    assert!(
        input.contains("M-1 - The lock is never released"),
        "{input}"
    );
    assert!(!input.contains("S-1"), "{input}");
    let copied = fs::read_to_string(prepared.workspace.join(".sbxm-task/issue.md")).unwrap();
    assert_eq!(copied, input);
    let prompt = fs::read_to_string(prepared.meta.join("worker-prompt.md")).unwrap();
    assert!(prompt.contains("pull request #7"), "{prompt}");
    assert!(!prompt.contains("Fixes #"), "{prompt}");
    let clone_head = git(&prepared.workspace, &["rev-parse", "HEAD"]);
    assert_eq!(clone_head, pr.head_oid);
}

#[test]
fn a_ready_fix_task_is_pushed_as_a_fast_forward_and_its_review_posted_as_the_new_heads() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    let repo_git = prepared.meta.join("repo.git");
    let tip = git(&repo_git, &["rev-parse", "refs/heads/feature-x"]);
    assert_ne!(tip, head);
    let github = FakeGitHub::default();

    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert_eq!(origin_head(&f), tip);
    assert_eq!(pushed.commit, tip);
    assert!(pushed.review_posted);
    assert_eq!(pushed.commits.len(), 1, "{:?}", pushed.commits);
    let calls = github.calls();
    let [GhCall::PrComment(repo, 7, body)] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(repo, "o/r");
    assert!(
        body.contains(&format!(
            "sbxm in-task review of PR #7's fix task, at commit {tip}"
        )),
        "{body}"
    );
    assert!(body.contains("Must-fix findings: 0"), "{body}");
    let task = Prepared::open(&f.env.base_dir(), &prepared.record.id)
        .unwrap()
        .record;
    assert_eq!(task.stage, Stage::Finished);
    assert_eq!(task.pr.as_deref(), Some("https://github.com/o/r/pull/7"));
    assert_eq!(task.review.unwrap().posted.as_deref(), Some(tip.as_str()));
}

/// Someone else pushes a commit to the PR's branch on the remote while the task runs.
fn push_from_elsewhere(f: &Fixture) -> String {
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
    git(&scratch, &["rev-parse", "HEAD"])
}

#[test]
fn a_pr_branch_that_moved_while_the_task_ran_is_not_pushed_and_the_result_is_kept() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    let moved = push_from_elsewhere(&f);
    let github = FakeGitHub::default();

    let message = format!(
        "{:#}",
        finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap_err()
    );

    let repo_git = prepared.meta.join("repo.git");
    assert!(
        message.contains(&format!(
            "PR #7's branch moved from {head} to {moved} while the task ran; the result is kept \
             in {} as feature-x; run `sbxm task fix --pr 7 --repo o/r` again on the new head",
            repo_git.display()
        )),
        "{message}"
    );
    assert_eq!(origin_head(&f), moved);
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    let task = Prepared::open(&f.env.base_dir(), &prepared.record.id)
        .unwrap()
        .record;
    assert_eq!((task.stage, task.pr), (Stage::Ready, None));
    // The result is still in the task's repo.git, on its local branch.
    let kept = git(&repo_git, &["rev-parse", "refs/heads/feature-x"]);
    assert_eq!(
        git(&repo_git, &["merge-base", "--is-ancestor", &head, &kept]),
        ""
    );
    assert_ne!(kept, head);
}

#[test]
fn a_fix_task_for_a_head_that_moved_before_the_fetch_is_refused_and_leaves_nothing() {
    let f = fixture();
    let (pr, review) = pr_with_review(&f);
    // The PR gets a new head after its review was chosen.
    let moved = add_pr_head(&f, 7);
    let id = record::fix_id(7, &pr.head_oid).unwrap();
    let backend = backend_for(&f, &id, vec![clean_review()]);
    let github = FakeGitHub::default();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);

    let message = format!(
        "{:#}",
        pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap_err()
    );

    assert!(
        message.contains(&format!(
            "the latest sbxm review covers {}, but PR #7's head is now {moved}; review the new \
             head first with `sbxm task review --pr 7 --repo o/r --base main`",
            pr.head_oid
        )),
        "{message}"
    );
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
    assert!(!f.env.base_dir().join("tasks").join(&id).exists());
    assert!(
        !backend.log().iter().any(|l| l.starts_with("create")),
        "{:?}",
        backend.log()
    );
}

#[test]
fn a_second_fix_of_the_same_head_is_refused_before_any_write() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    let pr = pr7(&head);
    let review = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(&head)]).unwrap();
    let backend = backend_for(&f, &prepared.record.id, vec![clean_review()]);
    let github = FakeGitHub::default();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);

    let message = format!(
        "{:#}",
        pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap_err()
    );

    assert!(
        message.contains(&format!(
            "task {} exists (stage ready): a fix of PR #7 at this head was started before",
            prepared.record.id
        )),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_review_that_looks_like_it_holds_a_secret_stops_the_push_and_names_where_the_result_is() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    fs::write(
        prepared.meta.join("review.md"),
        "Must-fix findings: 0\n\npassword = hunter2\n",
    )
    .unwrap();
    let github = FakeGitHub::default();

    let message = format!(
        "{:#}",
        finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap_err()
    );

    assert!(
        message.contains(&format!(
            "review.md line 3 looks like a secret (a password or token assignment), so nothing \
             was pushed to PR #7 and nothing posted; the result is kept in {} as feature-x",
            prepared.meta.join("repo.git").display()
        )),
        "{message}"
    );
    assert_eq!(origin_head(&f), head);
    assert!(github.calls().is_empty());
}

#[test]
fn task_finish_refuses_a_fix_task() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    let message = format!(
        "{:#}",
        sbxm::task::finish::finish(
            &f.env.base_dir(),
            &prepared.record.id,
            &FakeGitHub::default(),
            &Probe
        )
        .unwrap_err()
    );
    assert!(message.contains("is a fix task"), "{message}");
    assert_eq!(origin_head(&f), head);
}

fn merged(pr: &PrInfo) -> PrInfo {
    PrInfo {
        state: PrState::Merged,
        ..pr.clone()
    }
}

/// Review M-1: a fix task whose push was refused holds the only copy of its result, so the PR
/// merging later (with someone else's head) doesn't make it a `task rm --merged` candidate.
#[test]
fn an_unpushed_fix_result_is_left_alone_by_task_rm_merged_once_its_pr_merges() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    push_from_elsewhere(&f);
    finish_fix(
        &f.env.base_dir(),
        &prepared.record.id,
        &FakeGitHub::default(),
        &Probe,
    )
    .unwrap_err();
    let github = FakeGitHub::default().with_pr(merged(&pr7(&head)));

    let found = merged::plan(&f.env.base_dir(), &f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    let [left] = found.left_alone.as_slice() else {
        panic!("{found:?}");
    };
    assert_eq!(left.id, prepared.record.id);
    let repo_git = prepared.meta.join("repo.git");
    assert!(
        left.reason.contains(&format!(
            "its result was never pushed to PR #7's branch, so the PR being merged doesn't mean \
             it landed; it is kept in {} as feature-x",
            repo_git.display()
        )),
        "{}",
        left.reason
    );
    assert!(repo_git.is_dir());
}

/// A fix task whose result reached the PR's branch goes once the PR is merged.
#[test]
fn a_pushed_fix_task_is_a_task_rm_merged_candidate_once_its_pr_merges() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    finish_fix(
        &f.env.base_dir(),
        &prepared.record.id,
        &FakeGitHub::default(),
        &Probe,
    )
    .unwrap();
    let github = FakeGitHub::default().with_pr(merged(&pr7(&head)));

    let found = merged::plan(&f.env.base_dir(), &f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.left_alone.is_empty(), "{found:?}");
    let [candidate] = found.candidates.as_slice() else {
        panic!("{found:?}");
    };
    assert_eq!(candidate.id, prepared.record.id);
}

/// Issue 58: a merged task another live command holds is left alone, with that command named,
/// and keeps its folder.
#[test]
fn a_merged_task_another_command_holds_is_left_alone_by_rm_merged() {
    let f = fixture();
    let (prepared, head) = ready_fix(&f);
    finish_fix(
        &f.env.base_dir(),
        &prepared.record.id,
        &FakeGitHub::default(),
        &Probe,
    )
    .unwrap();
    let github = FakeGitHub::default().with_pr(merged(&pr7(&head)));
    let _held = sbxm::task::lock::ProcessLock::acquire(
        &prepared.meta,
        &format!("task {}", prepared.record.id),
        &Probe,
    )
    .unwrap();

    let found = merged::plan(&f.env.base_dir(), &f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    let [left] = found.left_alone.as_slice() else {
        panic!("{found:?}");
    };
    assert!(
        left.reason.contains(&format!(
            "task {} is in use by another sbxm command (process ",
            prepared.record.id
        )),
        "{}",
        left.reason
    );
    assert!(prepared.meta.join("task.json").is_file());
}
