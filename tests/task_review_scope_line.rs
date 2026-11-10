//! Issue 565: the review sbxm saves and posts carries sbxm's own `Scope:` line, from the base and
//! head commits it gave the reviewer, so a reviewer's typo in that line no longer keeps the
//! `sbxm/review` status from being posted.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, ctx, fixture, ok, play,
    play_reviews, source,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{PrComment, PrInfo, PrState, StatusState};
use sbxm::task::finish::finish_fix;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::pr_fix::review_to_fix;
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

/// The PR review the user posted on `head`, with one must-fix finding for the fix task to fix.
fn posted_review(head: &str) -> PrComment {
    let review = format!(
        "Must-fix findings: 1\nRisk: medium\n\nScope: `0000000..{head}`\nIssues: pending\n\n\
         ## Must fix\n\n### M-1 - The lock is never released\n\n**Where:** `src/lock.rs:10`\n\
         **What happens:** it stays held\n**Why it matters:** decision 1\n**Fix:** release it\n"
    );
    PrComment {
        author: "tutros".into(),
        body: pr_comment(7, "", &review),
    }
}

/// PR 7's fix task, its worker run and its review by a reviewer writing `review`: it ends
/// `ready`. Returns the task and PR 7's head, where the fix branch starts.
fn ready_fix(f: &Fixture, review: String) -> (Prepared, String) {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    let pr = pr7(&head);
    let to_fix = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(&head)]).unwrap();
    let id = record::fix_id(7, &pr.head_oid).unwrap();
    let worker = play(
        &f.env.base_dir().join("tasks").join(&id),
        "main",
        "feature-x",
        Play {
            commits: vec!["src/lock.rs".into()],
            result_md: Some(b"M-1 fixed\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![review]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        });
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_fix(&context, &pr, &to_fix, Location::Local).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    pipeline::review_issue(&context.env(), &mut prepared).unwrap();
    assert_eq!(
        (prepared.record.stage, prepared.record.status),
        (Stage::Ready, Status::Ok)
    );
    prepared.lock = None;
    (prepared, head)
}

fn tip_of(prepared: &Prepared) -> String {
    git(
        &prepared.meta.join("repo.git"),
        &["rev-parse", "refs/heads/feature-x"],
    )
}

fn scope_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("scope:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_scope_base_one_character_too_long_is_replaced_and_the_status_is_posted() {
    let f = fixture();
    // The reviewer knows the head but types the base with one character too many (PR #564).
    let typed = |tip: &str| {
        format!(
            "Must-fix findings: 0\nRisk: low\n\nScope: `d{}..{tip}`\n\nNothing found.\n",
            "a".repeat(40)
        )
    };
    // The worker's commit isn't known before it runs, so the head typed is a made-up one; sbxm's
    // line replaces both ends.
    let (prepared, base) = ready_fix(&f, typed(&"b".repeat(40)));
    let tip = tip_of(&prepared);
    let line = format!("Scope: `{base}..{tip}`");

    for name in ["review.md", "review-1.md"] {
        let saved = fs::read_to_string(prepared.meta.join(name)).unwrap();
        assert_eq!(
            scope_lines(&saved),
            std::slice::from_ref(&line),
            "{name}: {saved}"
        );
    }

    let github = FakeGitHub::default();
    let pushed = finish_fix(&f.env.base_dir(), &prepared.record.id, &github, &Probe).unwrap();

    assert!(pushed.review_posted);
    assert!(pushed.warnings.is_empty(), "{:?}", pushed.warnings);
    let calls = github.calls();
    let [
        GhCall::PrComment(_, _, comment),
        GhCall::CommitStatus(_, sha, status),
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_eq!(scope_lines(comment), [line]);
    assert_eq!(sha, &tip);
    assert_eq!(status.state, StatusState::Success);
}

#[test]
fn a_review_without_a_scope_line_is_saved_with_sbxms() {
    let f = fixture();
    let (prepared, base) = ready_fix(
        &f,
        "Must-fix findings: 0\nRisk: low\n\nNothing found.\n".to_owned(),
    );
    let tip = tip_of(&prepared);

    let saved = fs::read_to_string(prepared.meta.join("review.md")).unwrap();
    assert_eq!(scope_lines(&saved), [format!("Scope: `{base}..{tip}`")]);
}
