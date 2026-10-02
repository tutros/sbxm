//! M2b slice 9, step 4: reviewing a pull request (spec §5.2 "PR"): the gates run on a clean
//! checkout of the PR's head (the sandbox tier inside the reviewer's own sandbox: a PR has no
//! worker), the reviewer runs once, there is no fix round, and the review is posted as a PR
//! comment. Nothing is posted unless the review is valid.

mod common;

use std::fs;

use common::task_fixture::{
    CODEX_DONE, Fixture, add_pr_head, ctx, ctx_with_host, fixture_with, issue_text, ok,
    play_reviews, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{PrInfo, PrState};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, PrReview, Prepared};
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::review;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n1. must-fix: pr.txt:1 is wrong.\n";

fn config(host_gates: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\nhost = [{host_gates}]\n\n\
         [reviewer]\nharness = \"codex\"\n"
    ))
}

fn pr() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "Adds x.\n\nFixes #4".into(),
        closing_issues: vec![4],
    }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_pr(pr())
        .with_issue_text(issue_text(4))
}

fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(play_reviews(
            &f.env.base_dir(),
            "pr-7",
            reviews.iter().map(|s| (*s).to_owned()).collect(),
        ))
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "pr-7")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

fn prepared(f: &Fixture, backend: &FakeBackend, github: &FakeGitHub) -> Prepared {
    add_pr_head(f, 7);
    let source = source(f);
    pipeline::prepare_pr(&ctx(f, &source, backend, github), &pr()).unwrap()
}

fn review_it(
    f: &Fixture,
    backend: &FakeBackend,
    github: &FakeGitHub,
    host: &FakeHostRunner,
    prepared: &mut Prepared,
) -> anyhow::Result<PrReview> {
    let source = source(f);
    let context = ctx_with_host(f, &source, backend, github, host);
    pipeline::review_pr(&context.env(), github, prepared)
}

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

#[test]
fn a_pr_review_runs_gates_then_the_reviewer_posts_the_review_and_ends_ready() {
    let f = config("");
    let (backend, github) = (backend(&f, &[CLEAN]), github());
    let mut prepared = prepared(&f, &backend, &github);

    let report = review_it(
        &f,
        &backend,
        &github,
        &FakeHostRunner::default(),
        &mut prepared,
    )
    .unwrap();

    assert_eq!(
        report.reviewed.as_ref().map(|r| (r.round, r.must_fix)),
        Some((1, 0))
    );
    assert!(report.gates_failed.is_none() && report.posted);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(!record.fix_round && record.worker.is_none());
    assert_eq!(
        record.reviewer.as_ref().unwrap().sandbox,
        "sbxm-task-pr-7-review-codex"
    );
    // The review is saved under the reviewer's name, and posted as a comment on the PR.
    let saved_review = fs::read_to_string(meta(&f).join("review.md")).unwrap();
    assert!(
        saved_review.starts_with("Reviewer: codex (default model)\n\n"),
        "{saved_review}"
    );
    let calls = github.calls();
    let comment = calls.iter().find_map(|c| match c {
        GhCall::PrComment(repo, number, body) => Some((repo.clone(), *number, body.clone())),
        _ => None,
    });
    let (repo, number, body) = comment.expect("a comment was posted");
    assert_eq!((repo.as_str(), number), ("o/r", 7));
    assert!(
        body.contains("Nothing found.") && body.contains("Reviewer: codex"),
        "{body}"
    );
    // Reviewer sandbox and clone are gone; no worker clone ever existed.
    assert!(
        backend
            .removes()
            .contains(&"sbxm-task-pr-7-review-codex".to_owned())
    );
    assert!(!f.env.base_dir().join("tasks").join("pr-7-review").exists());
    assert!(!f.env.base_dir().join("tasks").join("pr-7").exists());
}

#[test]
fn the_sandbox_gates_run_inside_the_reviewers_sandbox_before_the_reviewer_does() {
    let f = config("");
    let (backend, github) = (backend(&f, &[CLEAN]), github());
    let mut prepared = prepared(&f, &backend, &github);

    review_it(
        &f,
        &backend,
        &github,
        &FakeHostRunner::default(),
        &mut prepared,
    )
    .unwrap();

    let execs = backend.execs();
    let gate = execs
        .iter()
        .position(|(_, s)| s.argv.iter().any(|a| a.ends_with("cargo test")))
        .unwrap();
    let reviewer = execs
        .iter()
        .position(|(_, s)| s.argv.iter().any(|a| a == "codex"))
        .unwrap();
    assert!(gate < reviewer, "gates first: {execs:?}");
    assert_eq!(
        execs[gate].0, "sbxm-task-pr-7-review-codex",
        "a PR has no worker sandbox"
    );
    assert_eq!(
        execs[gate].1.workdir.as_deref(),
        Some(
            sbxm::run::orchestrate::in_sandbox_path(
                &f.env.base_dir().join("tasks").join("pr-7-review")
            )
            .as_path()
        ),
        "on the reviewer's clone of the PR head"
    );
    assert_eq!(
        backend.creates().len(),
        1,
        "one sandbox only: the reviewer's"
    );
    assert_eq!(saved(&f).gates.len(), 1);
    assert_eq!(saved(&f).gates[0].phase, "pr");
}

#[test]
fn there_is_no_fix_round_even_with_must_fix_findings() {
    let f = config("");
    let (backend, github) = (backend(&f, &[ONE]), github());
    let mut prepared = prepared(&f, &backend, &github);

    let report = review_it(
        &f,
        &backend,
        &github,
        &FakeHostRunner::default(),
        &mut prepared,
    )
    .unwrap();

    assert_eq!(report.reviewed.unwrap().must_fix, 1);
    assert_eq!(count(&backend, "codex"), 1, "one review only");
    assert_eq!(count(&backend, "fix-prompt.md"), 0);
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(
        github.calls().iter().any(
            |c| matches!(c, GhCall::PrComment(_, 7, body) if body.contains("pr.txt:1 is wrong"))
        )
    );
}

#[test]
fn failing_gates_stop_the_review_nothing_is_posted_and_everything_is_removed() {
    let f = config("");
    let github = github();
    let ok_backend = backend(&f, &[CLEAN]);
    let mut prepared = prepared(&f, &ok_backend, &github);
    let red = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(101),
        },
    );

    let report = review_it(&f, &red, &github, &FakeHostRunner::default(), &mut prepared).unwrap();

    assert_eq!(report.gates_failed.as_ref().unwrap().command, "cargo test");
    assert!(report.reviewed.is_none() && !report.posted);
    assert_eq!(count(&red, "codex"), 0, "the reviewer never ran");
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrComment(..)))
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::GatesFailed)
    );
    assert!(
        red.removes()
            .contains(&"sbxm-task-pr-7-review-codex".to_owned())
    );
    assert!(!f.env.base_dir().join("tasks").join("pr-7-review").exists());
    assert!(
        fs::read_to_string(meta(&f).join("gates.log"))
            .unwrap()
            .contains("cargo test")
    );
}

#[test]
fn host_gates_run_on_a_clean_checkout_of_the_prs_head_and_only_after_the_sandbox_tier() {
    let f = config("\"cargo build\"");
    let github = github();
    let backend = backend(&f, &[CLEAN]);
    let mut prepared = prepared(&f, &backend, &github);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_in_hook = std::sync::Arc::clone(&seen);
    let host = FakeHostRunner::default().with_hook(move |cwd, command| {
        seen_in_hook.lock().unwrap().push((
            cwd.to_path_buf(),
            command.to_owned(),
            cwd.join("pr.txt").is_file(),
        ));
    });

    review_it(&f, &backend, &github, &host, &mut prepared).unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, f.env.base_dir().join("tasks").join("pr-7-gates"));
    assert!(seen[0].2, "the PR's own file is in the checkout");
    assert!(
        !f.env.base_dir().join("tasks").join("pr-7-gates").exists(),
        "removed after"
    );
    assert_eq!(
        saved(&f).gates.iter().filter(|g| g.tier == "host").count(),
        1
    );
}

#[test]
fn an_invalid_review_posts_nothing_and_fails_the_task_review() {
    let f = config("");
    let (backend, github) = (backend(&f, &["Looks fine to me.\n"]), github());
    let mut prepared = prepared(&f, &backend, &github);

    let message = format!(
        "{:#}",
        review_it(
            &f,
            &backend,
            &github,
            &FakeHostRunner::default(),
            &mut prepared
        )
        .unwrap_err()
    );

    assert!(
        message.contains("Must-fix findings: <count>") && message.contains("isn't used"),
        "{message}"
    );
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrComment(..)))
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Failed)
    );
    assert!(
        backend
            .removes()
            .contains(&"sbxm-task-pr-7-review-codex".to_owned())
    );
    assert!(!f.env.base_dir().join("tasks").join("pr-7-review").exists());
}

#[test]
fn a_failed_post_keeps_the_review_and_says_where_it_is() {
    let f = config("");
    let (backend, github) = (
        backend(&f, &[CLEAN]),
        github().failing_comments("rate limited"),
    );
    let mut prepared = prepared(&f, &backend, &github);

    let message = format!(
        "{:#}",
        review_it(
            &f,
            &backend,
            &github,
            &FakeHostRunner::default(),
            &mut prepared
        )
        .unwrap_err()
    );

    assert!(
        message.contains("rate limited") && message.contains("review.md"),
        "{message}"
    );
    assert!(message.contains("gh pr comment 7"), "{message}");
    assert!(meta(&f).join("review.md").is_file(), "the review is saved");
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(
        record.notes.iter().any(|n| n.contains("not posted")),
        "{:?}",
        record.notes
    );
}

#[test]
fn a_task_that_is_already_reviewed_is_refused() {
    let f = config("");
    let (backend, github) = (backend(&f, &[CLEAN]), github());
    let mut prepared = prepared(&f, &backend, &github);
    review_it(
        &f,
        &backend,
        &github,
        &FakeHostRunner::default(),
        &mut prepared,
    )
    .unwrap();

    let message = format!(
        "{:#}",
        review_it(
            &f,
            &backend,
            &github,
            &FakeHostRunner::default(),
            &mut prepared
        )
        .unwrap_err()
    );

    assert!(
        message.contains("already") && message.contains("ready"),
        "{message}"
    );
}

// ---- The comment text ----

#[test]
fn the_comment_names_the_pr_and_keeps_the_review() {
    let text = review::pr_comment(
        7,
        "Reviewer: codex (m)\n\nMust-fix findings: 0\n\nAll good.\n",
    );
    assert!(text.starts_with("sbxm review of PR #7"), "{text}");
    assert!(text.contains("All good."), "{text}");
}

#[test]
fn mentions_in_the_review_do_not_ping_anyone() {
    let text = review::pr_comment(7, "Ask @someone and @team/leads; mail a@b.com; (@x) too.\n");
    assert!(
        !text.contains("@someone") && !text.contains("@team") && !text.contains("(@x)"),
        "{text}"
    );
    assert!(
        text.contains("someone") && text.contains("a@b.com"),
        "{text}"
    );
}

#[test]
fn a_very_long_review_is_cut_to_what_github_accepts_and_the_comment_says_so() {
    let long = "x".repeat(100_000);
    let text = review::pr_comment(7, &long);
    assert!(text.chars().count() < 65_536, "{}", text.chars().count());
    assert!(
        text.contains("cut") && text.contains("review.md"),
        "{}",
        &text[text.len() - 300..]
    );
}
