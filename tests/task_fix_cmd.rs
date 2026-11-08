//! Issue 313: `sbxm task fix --pr N` as a command, through each boundary case the issue lists.
//! The happy path runs the worker, the gates and the in-task review and pushes the result as a
//! fast-forward, leaving a review of the new head on the PR; every refusal but the push's comes
//! before anything is written or any sandbox call is made.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, fixture_with, ok, play,
    play_reviews,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_fix::{Options, run};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pr_fix::review_to_fix;
use sbxm::task::record;
use sbxm::task::repo::Identity;
use sbxm::task::review::pr_comment;

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

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

fn review_text(head: &str, must_fix: u32) -> String {
    let mut text = format!(
        "Must-fix findings: {must_fix}\nRisk: medium\n\nScope: `0000000..{head}`\n\
         Issues: pending\n\n"
    );
    if must_fix > 0 {
        text.push_str(
            "## Must fix\n\n### M-1 - The lock is never released\n\n\
             **Where:** `src/lock.rs:10`\n**What happens:** it stays held\n\
             **Why it matters:** decision 1\n**Fix:** release it\n\n",
        );
    }
    text.push_str(
        "## Should fix\n\n### S-1 - A name is unclear\n\n**Where:** `src/name.rs:1`\n\
         **What happens:** x\n**Why it matters:** y\n**Fix:** rename it\n",
    );
    text
}

fn posted_by(author: &str, head: &str, must_fix: u32) -> PrComment {
    PrComment {
        author: author.into(),
        body: pr_comment(7, "", &review_text(head, must_fix)),
    }
}

/// PR 7's head on the remote (branch `feature-x`).
fn pr_head(f: &Fixture) -> String {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    head
}

fn github(pr: PrInfo, comments: Vec<PrComment>) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(pr)
        .with_pr_comments(7, comments)
}

/// The in-task reviewer's text: clean.
const CLEAN: &str = "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing.\n";

fn backend(f: &Fixture, head: &str, on_review: impl Fn() + Send + Sync + 'static) -> FakeBackend {
    let id = record::fix_id(7, head).unwrap();
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
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN.to_owned()]);
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            if sandbox.contains("-review-") && exec.argv.iter().any(|a| a == "codex") {
                on_review();
            }
            reviewer(sandbox, exec);
        })
}

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        pr: 7,
        repo: Some("o/r".into()),
        base: None,
        clone_source: Some(f.origin.to_str().unwrap().to_owned()),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        check_run: None,
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
}

fn go(f: &Fixture, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        &options(f),
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}

/// A refusal: its message, with nothing written under the base dir and no sandbox call made.
fn refused(f: &Fixture, github: &FakeGitHub) -> String {
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let out = go(f, &backend, github);
    let message = format!("{:#}", out.result.unwrap_err());
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    let tasks = f.env.base_dir().join("tasks");
    assert!(
        !tasks.exists() || fs::read_dir(&tasks).unwrap().next().is_none(),
        "{message}"
    );
    assert!(
        !record::tasks_root(&f.env.base_dir()).exists()
            || fs::read_dir(record::tasks_root(&f.env.base_dir()))
                .unwrap()
                .next()
                .is_none(),
        "{message}"
    );
    message
}

// ---- The machine's sandbox slot (issue 504, decision 304) ----

#[test]
fn a_fix_with_no_free_slot_and_no_wait_refuses_creating_nothing() {
    let f = config();
    let head = pr_head(&f);
    let github = github(pr7(&head), vec![posted_by("tutros", &head, 1)]);
    let backend = backend(&f, &head, || {});

    let result = run(
        &f.env.config_dir(),
        &Options {
            disk: common::slot::full(&f.env, true),
            ..options(&f)
        },
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.starts_with(&common::slot::refusal()), "{err}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!record::tasks_root(&f.env.base_dir()).exists());
    assert_eq!(origin_head(&f), head);
}

#[test]
fn a_fix_holds_the_slot_while_it_runs_and_gives_it_back() {
    let f = config();
    let head = pr_head(&f);
    let github = github(pr7(&head), vec![posted_by("tutros", &head, 1)]);
    let env_slot = common::slot::slot_file(&f.env);
    let backend = backend(&f, &head, move || {
        assert!(env_slot.exists(), "the reviewer ran without the slot");
    });

    let result = run(
        &f.env.config_dir(),
        &Options {
            disk: common::slot::free(&f.env),
            ..options(&f)
        },
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    result.unwrap();
    assert!(common::slot::holder(&f.env).is_none());
}

#[test]
fn one_must_fix_is_fixed_gated_reviewed_and_pushed_and_the_pr_gets_a_review_of_the_new_head() {
    let f = config();
    let head = pr_head(&f);
    let github = github(pr7(&head), vec![posted_by("tutros", &head, 1)]);
    let backend = backend(&f, &head, || {});

    let out = go(&f, &backend, &github);

    out.result.unwrap();
    let id = record::fix_id(7, &head).unwrap();
    let tip = origin_head(&f);
    assert_ne!(tip, head);
    // A fast-forward: the old head is the new one's ancestor.
    git(&f.origin, &["merge-base", "--is-ancestor", &head, &tip]);
    assert!(out.out.contains(&format!("{id}: worker")), "{}", out.out);
    assert!(out.out.contains("review round 1"), "{}", out.out);
    assert!(
        out.out.contains(&format!(
            "{id}: pushed 1 commit(s) onto PR #7's branch feature-x, now at {tip}"
        )),
        "{}",
        out.out
    );
    // The gates ran and passed before the review.
    let task = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert!(
        task.gates
            .iter()
            .any(|g| g.command == "cargo test" && g.passed),
        "{:?}",
        task.gates
    );
    assert_eq!(task.stage, record::Stage::Finished);
    let posted: Vec<String> = github
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::PrComment(_, 7, body) => Some(body),
            _ => None,
        })
        .collect();
    let [body] = posted.as_slice() else {
        panic!("{posted:?}");
    };
    assert!(
        body.contains(&format!(
            "sbxm in-task review of PR #7's fix task, at commit {tip}"
        )),
        "{body}"
    );
    // That comment is the user's sbxm review of the new head, so it counts as the PR's review.
    let review_of_tip = PrComment {
        author: "tutros".into(),
        body: body.replace("aaaaaaa..bbbbbbb", &format!("{}..{tip}", &head[..7])),
    };
    let message = format!(
        "{:#}",
        review_to_fix(&pr7(&tip), "o/r", None, "tutros", &[review_of_tip]).unwrap_err()
    );
    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {tip}) has no must-fix findings"
        )),
        "{message}"
    );
}

#[test]
fn a_review_with_no_must_fix_is_refused_before_any_write() {
    let f = config();
    let head = pr_head(&f);
    let message = refused(&f, &github(pr7(&head), vec![posted_by("tutros", &head, 0)]));
    assert!(
        message.contains(&format!(
            "the latest sbxm review of PR #7 (commit {head}) has no must-fix findings, so there \
             is nothing to fix; file its other findings with `sbxm task file-findings`"
        )),
        "{message}"
    );
}

#[test]
fn a_head_that_moved_by_one_commit_after_the_review_is_refused_at_the_start() {
    let f = config();
    let reviewed = pr_head(&f);
    let head = pr_head(&f);
    let message = refused(
        &f,
        &github(pr7(&head), vec![posted_by("tutros", &reviewed, 1)]),
    );
    assert!(
        message.contains(&format!(
            "the latest sbxm review covers {reviewed}, but PR #7's head is now {head}; review \
             the new head first with `sbxm task review --pr 7 --repo o/r`"
        )),
        "{message}"
    );
}

#[test]
fn a_head_that_moved_by_one_commit_while_the_task_ran_is_not_pushed() {
    let f = config();
    let head = pr_head(&f);
    let github = github(pr7(&head), vec![posted_by("tutros", &head, 1)]);
    let elsewhere = f.env.tmp.path().join("elsewhere");
    let origin = f.origin.clone();
    let scratch = elsewhere.clone();
    // Someone pushes to the PR's branch while the in-task reviewer runs.
    let backend = backend(&f, &head, move || {
        if scratch.exists() {
            return;
        }
        git(
            origin.parent().unwrap(),
            &[
                "clone",
                "-q",
                "-b",
                "feature-x",
                origin.to_str().unwrap(),
                scratch.to_str().unwrap(),
            ],
        );
        fs::write(scratch.join("other.txt"), "someone else\n").unwrap();
        git(&scratch, &["add", "-A"]);
        git(&scratch, &["commit", "-q", "-m", "someone else's change"]);
        git(&scratch, &["push", "-q", "origin", "feature-x"]);
    });

    let out = go(&f, &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    let moved = git(&elsewhere, &["rev-parse", "HEAD"]);
    let id = record::fix_id(7, &head).unwrap();
    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    assert!(
        message.contains(&format!(
            "PR #7's branch moved from {head} to {moved} while the task ran; the result is kept \
             in {} as feature-x; run `sbxm task fix --pr 7 --repo o/r` again on the new head",
            repo_git.display()
        )),
        "{message}"
    );
    assert_eq!(origin_head(&f), moved);
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrComment(..))),
        "{:?}",
        github.calls()
    );
}

#[test]
fn a_review_with_the_header_posted_by_someone_else_is_ignored() {
    let f = config();
    let head = pr_head(&f);
    let message = refused(
        &f,
        &github(pr7(&head), vec![posted_by("mallory", &head, 1)]),
    );
    assert!(
        message.contains(
            "PR #7 has no sbxm review by tutros; run `sbxm task review --pr 7 --repo o/r` first"
        ),
        "{message}"
    );
}

#[test]
fn a_pr_with_no_sbxm_review_is_refused() {
    let f = config();
    let head = pr_head(&f);
    let message = refused(&f, &github(pr7(&head), vec![]));
    assert!(
        message.contains(
            "PR #7 has no sbxm review by tutros; run `sbxm task review --pr 7 --repo o/r` first"
        ),
        "{message}"
    );
}

#[test]
fn a_fork_pr_is_refused_before_its_comments_are_read() {
    let f = config();
    let head = pr_head(&f);
    let mut fork = pr7(&head);
    fork.is_cross_repository = true;
    fork.head_repo = "someone/r".into();
    let github = github(fork, vec![posted_by("tutros", &head, 1)]);
    let message = refused(&f, &github);
    assert!(
        message.contains(
            "PR #7 comes from a fork (someone/r); sbxm can push only to this repo's own \
             branches; `sbxm task review --pr 7 --repo o/r` still works"
        ),
        "{message}"
    );
    assert_eq!(github.calls(), [GhCall::Pr("o/r".into(), 7)]);
}

#[test]
fn closed_and_merged_prs_are_refused() {
    for (state, word) in [(PrState::Closed, "closed"), (PrState::Merged, "merged")] {
        let f = config();
        let head = pr_head(&f);
        let mut done = pr7(&head);
        done.state = state;
        let message = refused(&f, &github(done, vec![posted_by("tutros", &head, 1)]));
        assert!(
            message.contains(&format!(
                "PR #7 is {word}; there is no open branch to push to"
            )),
            "{message}"
        );
    }
}
