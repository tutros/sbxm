//! Issue 469: `task fix --pr` posts the `sbxm/review` status on the new head right after the
//! in-task review comment, through the same function the other two review posts use.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, ctx, fixture, ok, play,
    play_reviews, source,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{CommitStatus, PrComment, PrInfo, PrState, StatusState};
use sbxm::task::finish::finish_fix;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::pr_fix::{ReviewToFix, review_to_fix};
use sbxm::task::record::{self, Stage, Status};
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
        base_moved: false,
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
    // The fake reviewer's text carries a made-up Scope; a real one ends at the commit it reviewed.
    let tip = tip_of(&prepared);
    fs::write(
        prepared.meta.join("review.md"),
        format!("Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..{tip}`\n\nNothing found.\n"),
    )
    .unwrap();
    (prepared, pr.head_oid)
}

fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}

fn statuses(github: &FakeGitHub) -> Vec<(String, CommitStatus)> {
    github
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::CommitStatus(_, sha, status) => Some((sha, status)),
            _ => None,
        })
        .collect()
}

fn tip_of(prepared: &Prepared) -> String {
    git(
        &prepared.meta.join("repo.git"),
        &["rev-parse", "refs/heads/feature-x"],
    )
}

#[test]
fn a_clean_fix_review_is_followed_by_a_success_status_on_the_new_head() {
    let f = fixture();
    let (prepared, _) = ready_fix(&f);
    let tip = tip_of(&prepared);
    let github = FakeGitHub::default();

    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert!(pushed.review_posted);
    assert!(pushed.warnings.is_empty(), "{:?}", pushed.warnings);
    let calls = github.calls();
    let [
        GhCall::PrComment(..),
        GhCall::CommitStatus(repo, sha, status),
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_eq!((repo.as_str(), sha.as_str()), ("o/r", tip.as_str()));
    assert_eq!(status.state, StatusState::Success);
    assert_eq!(status.context, "sbxm/review");
}

#[test]
fn a_fix_review_with_must_fix_left_is_followed_by_a_failure_status_on_the_new_head() {
    let f = fixture();
    let (mut prepared, _) = ready_fix(&f);
    let tip = tip_of(&prepared);
    prepared.record.review.as_mut().unwrap().must_fix = 2;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    let [(sha, status)] = statuses(&github).try_into().unwrap();
    assert_eq!(sha, tip);
    assert_eq!(status.state, StatusState::Failure);
    assert!(status.description.starts_with("2 must-fix"), "{status:?}");
}

#[test]
fn a_failing_status_post_is_a_warning_and_the_fix_is_still_pushed_and_posted() {
    let f = fixture();
    let (prepared, _) = ready_fix(&f);
    let tip = tip_of(&prepared);
    let github = FakeGitHub::default().failing_statuses("boom");

    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert_eq!(origin_head(&f), tip);
    assert!(pushed.review_posted);
    let [warning] = pushed.warnings.as_slice() else {
        panic!("{:?}", pushed.warnings);
    };
    assert!(warning.contains("boom"), "{warning}");
    assert!(
        warning.contains("task review --pr 7") && warning.contains("--force"),
        "{warning}"
    );
    let task = Prepared::open(&f.env.base_dir(), &prepared.record.id)
        .unwrap()
        .record;
    assert_eq!(task.review.unwrap().posted.as_deref(), Some(tip.as_str()));
}

#[test]
fn a_review_whose_scope_doesnt_end_at_the_new_head_posts_no_status_and_warns() {
    let f = fixture();
    let (prepared, _) = ready_fix(&f);
    fs::write(
        prepared.meta.join("review.md"),
        "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..1234567`\n\nNothing found.\n",
    )
    .unwrap();
    let github = FakeGitHub::default();

    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert!(pushed.review_posted);
    assert!(statuses(&github).is_empty());
    let [warning] = pushed.warnings.as_slice() else {
        panic!("{:?}", pushed.warnings);
    };
    assert!(warning.contains("Scope ends at 1234567"), "{warning}");
}

#[test]
fn no_status_is_posted_when_no_review_covered_the_new_head() {
    let f = fixture();
    let (mut prepared, _) = ready_fix(&f);
    prepared.record.review.as_mut().unwrap().commit = Some("0".repeat(40));
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert!(!pushed.review_posted);
    assert!(pushed.warnings.is_empty());
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}

#[test]
fn a_rerun_after_a_failed_comment_post_posts_the_status_with_the_comment() {
    let f = fixture();
    let (prepared, _) = ready_fix(&f);
    let tip = tip_of(&prepared);
    let failing = FakeGitHub::default().failing_comments("down");
    finish_fix(&f.env.base_dir(), &prepared.record.id, &failing, &Probe).unwrap_err();
    assert!(statuses(&failing).is_empty());
    // The failed run advanced the task to `finished`; the rerun starts from `ready` again.
    let mut again = Prepared::open(&f.env.base_dir(), &prepared.record.id).unwrap();
    again.record.stage = Stage::Ready;
    again.record.pr = None;
    record::write(&again.meta, &again.record).unwrap();
    let github = FakeGitHub::default();

    finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    let [(sha, status)] = statuses(&github).try_into().unwrap();
    assert_eq!((sha, status.state), (tip, StatusState::Success));
}
