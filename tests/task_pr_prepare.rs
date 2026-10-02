//! M2b slice 9, step 1: preparing a task for a pull request (spec §5.2 "PR"): same-repo open PRs
//! only, `repo.git` with the PR's head fetched, the PR and the issues it closes as the review
//! context, a link to an issue task that exists; every refusal happens before a write.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{Fixture, add_pr_head, ctx, fixture_with, issue_text, source};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::pipeline;
use sbxm::task::record::{self, Kind, NewTask, Process, Stage, Status};

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn pr(number: u32) -> PrInfo {
    PrInfo {
        number,
        head_ref: "feature-x".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.\n\nFixes #4".into(),
        closing_issues: vec![4],
    }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_pr(pr(7))
        .with_issue_text(issue_text(4))
}

fn backend() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "pr-7")
}

fn nothing_new(f: &Fixture) -> bool {
    let empty_or_absent = |dir: std::path::PathBuf| {
        fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
    };
    empty_or_absent(f.env.base_dir().join("tasks"))
        && empty_or_absent(f.env.base_dir().join(".sbxm").join("tasks"))
}

#[test]
fn a_prepared_pr_task_has_the_prs_head_the_review_context_and_a_record() {
    let f = config();
    let sha = add_pr_head(&f, 7);
    let (backend, github) = (backend(), github());
    let source = source(&f);

    let prepared = pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap();

    assert_eq!(prepared.meta, meta(&f));
    let repo_git = meta(&f).join("repo.git");
    assert_eq!(git(&repo_git, &["rev-parse", "refs/heads/pr-7"]), sha);

    // The review context: the PR (title, branch, description) and the issue it closes.
    let context = fs::read_to_string(meta(&f).join("issue.md")).unwrap();
    assert!(
        context.contains("Pull request #7") && context.contains("Add the x feature"),
        "{context}"
    );
    assert!(
        context.contains("feature-x") && context.contains("This adds x."),
        "{context}"
    );
    assert!(
        context.contains("Issue #4") && context.contains("**Acceptance criteria:** do it"),
        "{context}"
    );

    let record = record::read(&meta(&f).join("task.json")).unwrap();
    assert_eq!(
        (record.id.as_str(), record.kind, record.number),
        ("pr-7", Kind::Pr, 7)
    );
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Running)
    );
    assert_eq!(
        (record.branch.as_str(), record.base.as_str()),
        ("pr-7", "main")
    );
    assert_eq!(record.title, "Add the x feature");
    assert!(record.worker.is_none() && record.related.is_empty());
    assert_eq!(prepared.record, record);
    // No worker clone, and no sandbox: the reviewer's are made when the review runs.
    assert!(!f.env.base_dir().join("tasks").join("pr-7").exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn a_closing_issue_that_has_a_task_is_linked() {
    let f = config();
    add_pr_head(&f, 7);
    // An issue task for #4 already exists.
    let issue_task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 4,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "issue-4",
            config_hash: "h",
        },
        0,
        Process::new(1, 0),
    );
    record::write(&record::task_dir(&f.env.base_dir(), "issue-4"), &issue_task).unwrap();
    let (backend, github) = (backend(), github());
    let source = source(&f);

    let prepared = pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap();

    assert_eq!(prepared.record.related, [4]);
}

#[test]
fn a_pr_from_a_fork_is_refused_before_anything_is_written() {
    let f = config();
    let (backend, github) = (backend(), github());
    let source = source(&f);
    let mut fork = pr(7);
    fork.is_cross_repository = true;

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &fork).unwrap_err()
    );

    assert!(
        message.contains("fork") && message.contains("#7"),
        "{message}"
    );
    assert!(nothing_new(&f));
}

#[test]
fn a_closed_or_merged_pr_is_refused_before_anything_is_written() {
    for (state, word) in [(PrState::Closed, "closed"), (PrState::Merged, "merged")] {
        let f = config();
        let (backend, github) = (backend(), github());
        let source = source(&f);
        let mut done = pr(7);
        done.state = state;

        let message = format!(
            "{:#}",
            pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &done).unwrap_err()
        );

        assert!(
            message.contains(word) && message.contains("only open"),
            "{message}"
        );
        assert!(nothing_new(&f));
    }
}

#[test]
fn an_existing_task_for_the_pr_is_refused_and_left_alone() {
    let f = config();
    add_pr_head(&f, 7);
    let (backend, github) = (backend(), github());
    let source = source(&f);
    pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap();
    let before = fs::read_to_string(meta(&f).join("task.json")).unwrap();

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap_err()
    );

    assert!(
        message.contains("task pr-7 exists (stage prepared)"),
        "{message}"
    );
    assert!(
        message.contains("task status") && message.contains("task rm --pr 7"),
        "{message}"
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("task.json")).unwrap(),
        before
    );
}

#[test]
fn the_reviewers_secret_is_checked_before_anything_is_written() {
    let f = config();
    let (backend, github) = (FakeBackend::with_secrets(&["anthropic"]), github());
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap_err()
    );

    assert!(
        message.contains("openai") && message.contains("sbx secret set openai"),
        "{message}"
    );
    assert!(nothing_new(&f));
}

#[test]
fn a_pr_head_that_cannot_be_fetched_leaves_nothing_behind() {
    let f = config();
    // No refs/pull/7/head exists in the stand-in origin.
    let (backend, github) = (backend(), github());
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap_err()
    );

    assert!(message.contains("PR #7"), "{message}");
    assert!(nothing_new(&f));
}

#[test]
fn a_linked_issue_that_cannot_be_read_is_an_error_and_leaves_nothing_behind() {
    let f = config();
    add_pr_head(&f, 7);
    // The PR closes #4 but GitHub can't show it.
    let (backend, github) = (backend(), FakeGitHub::default().with_pr(pr(7)));
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap_err()
    );

    assert!(
        message.contains("issue #4") || message.contains("issue 4"),
        "{message}"
    );
    assert!(nothing_new(&f));
}

#[test]
fn an_invalid_reviewer_kit_leaves_nothing_behind() {
    let f = config();
    add_pr_head(&f, 7);
    let backend = FakeBackend::with_invalid_kit("bad mixin").and_secrets(&["anthropic", "openai"]);
    let github = github();
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare_pr(&ctx(&f, &source, &backend, &github), &pr(7)).unwrap_err()
    );

    assert!(message.contains("bad mixin"), "{message}");
    assert!(nothing_new(&f));
}
