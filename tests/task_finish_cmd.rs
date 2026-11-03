//! M2b slice 10, step 6: `sbxm task finish` as a command.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::record::{self, Process, Stage, Status};

fn ready(f: &Fixture) {
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
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), "Must-fix findings: 0\n").unwrap();
}

fn go(f: &Fixture, issue: u32, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Issue(issue),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn it_prints_the_pr_url_and_the_next_step() {
    let f = fixture();
    ready(&f);

    let (result, out) = go(&f, 41, &FakeGitHub::default());

    result.unwrap();
    assert!(
        out.contains("issue-41: pushed issue-41 and opened https://github.com/o/r/pull/"),
        "{out}"
    );
    assert!(out.contains("sbxm task rm --issue 41"), "{out}");
}

#[test]
fn a_draft_pr_is_called_a_draft_with_what_is_left() {
    let f = fixture();
    ready(&f);
    let meta = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-41");
    fs::write(meta.join("review.md"), "Must-fix findings: 1\n").unwrap();

    let (result, out) = go(&f, 41, &FakeGitHub::default());

    result.unwrap();
    assert!(
        out.contains("issue-41: pushed issue-41 and opened draft PR https://github.com/o/r/pull/"),
        "{out}"
    );
    assert!(out.contains("1 must-fix finding(s) left"), "{out}");
}

#[test]
fn issue_zero_and_an_unknown_task_are_refused_with_a_fix() {
    let f = fixture();

    let (zero, _) = go(&f, 0, &FakeGitHub::default());
    let (unknown, _) = go(&f, 41, &FakeGitHub::default());

    assert!(format!("{:#}", zero.unwrap_err()).contains("pass --issue <n>"));
    assert!(
        format!("{:#}", unknown.unwrap_err()).contains("no task issue-41; run `sbxm task status`")
    );
}

/// Issue 198: a task whose recorded review covered the PR's head says the review is posted as
/// the PR's own, and that a later commit gets `task review --pr`.
#[test]
fn a_posted_in_task_review_is_reported_with_its_commit() {
    let f = fixture();
    ready(&f);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut task = record::read(&meta.join("task.json")).unwrap();
    let tip = common::git(&meta.join("repo.git"), &["rev-parse", "issue-41"]);
    task.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(tip.clone()),
        ..Default::default()
    });
    // Not the checkout's origin: the printed command must name the recorded repo and base.
    task.repo = "owner/target".into();
    record::write(&meta, &task).unwrap();

    let (result, out) = go(&f, 41, &FakeGitHub::default());

    result.unwrap();
    assert!(
        out.contains(&format!(
            "the in-task review of {tip} is posted on PR #9 as its review"
        )),
        "{out}"
    );
    assert!(
        out.contains("sbxm task review --pr 9 --repo owner/target --base main"),
        "{out}"
    );
}

/// Issue 317: a PR whose head no review covered gets no `sbxm/review` status, and the command
/// says how to review it.
#[test]
fn a_pr_whose_head_is_not_the_reviewed_commit_gets_no_status_and_the_review_command() {
    let f = fixture();
    ready(&f);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut task = record::read(&meta.join("task.json")).unwrap();
    let base = common::git(&meta.join("repo.git"), &["rev-parse", "main"]);
    task.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(base),
        ..Default::default()
    });
    record::write(&meta, &task).unwrap();
    let github = FakeGitHub::default();

    let (result, out) = go(&f, 41, &github);

    result.unwrap();
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::CommitStatus(..)))
    );
    assert!(
        out.contains(
            "no review covers PR #9's head, so it has no sbxm/review status; review it with: \
             sbxm task review --pr 9 --repo o/r --base main"
        ),
        "{out}"
    );
}

/// Issue 317: a status that can't be posted is a warning, and the finish still succeeds.
#[test]
fn a_failed_status_is_printed_as_a_warning() {
    let f = fixture();
    ready(&f);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut task = record::read(&meta.join("task.json")).unwrap();
    let tip = common::git(&meta.join("repo.git"), &["rev-parse", "issue-41"]);
    task.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(tip.clone()),
        ..Default::default()
    });
    record::write(&meta, &task).unwrap();
    let base = common::git(&meta.join("repo.git"), &["rev-parse", "main"]);
    fs::write(
        meta.join("review.md"),
        format!("Must-fix findings: 0\n\nScope: `{base}..{tip}`\n"),
    )
    .unwrap();

    let (result, out) = go(&f, 41, &FakeGitHub::default().failing_statuses("HTTP 403"));

    result.unwrap();
    assert!(
        out.contains(&format!(
            "warning: the sbxm/review status on commit {tip} couldn't be posted"
        )),
        "{out}"
    );
}

/// Issue 317, review M-1: a review whose `Scope:` ends at another commit certifies nothing, so
/// the PR's head gets no status, and the warning says why and how to review it again.
#[test]
fn a_review_scoped_to_another_commit_posts_no_status_and_says_why() {
    let f = fixture();
    ready(&f);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut task = record::read(&meta.join("task.json")).unwrap();
    let tip = common::git(&meta.join("repo.git"), &["rev-parse", "issue-41"]);
    let base = common::git(&meta.join("repo.git"), &["rev-parse", "main"]);
    task.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(tip.clone()),
        ..Default::default()
    });
    record::write(&meta, &task).unwrap();
    fs::write(
        meta.join("review.md"),
        format!("Must-fix findings: 0\n\nScope: `{base}..{base}`\n"),
    )
    .unwrap();
    let github = FakeGitHub::default();

    let (result, out) = go(&f, 41, &github);

    result.unwrap();
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::CommitStatus(..)))
    );
    assert!(
        out.contains(&format!(
            "warning: the sbxm/review status wasn't posted: the review's Scope ends at {base}, \
             not at the reviewed commit {tip}"
        )),
        "{out}"
    );
    assert!(
        out.contains("`sbxm task review --pr 9 --repo o/r --base main --force`"),
        "{out}"
    );
}

// ---- The task's lock (issue 58) ----

#[test]
fn a_finish_is_refused_while_another_live_command_holds_the_task() {
    let f = fixture();
    ready(&f);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let _held = sbxm::task::lock::ProcessLock::acquire(&meta, "task issue-41", &Probe).unwrap();
    let before = fs::read(meta.join("task.json")).unwrap();
    let github = FakeGitHub::default();

    let (result, _) = go(&f, 41, &github);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("task issue-41 is in use by another sbxm command (process "),
        "{message}"
    );
    assert_eq!(fs::read(meta.join("task.json")).unwrap(), before);
    assert!(github.calls().is_empty(), "nothing was published");
}
