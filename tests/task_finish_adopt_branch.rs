//! Issue 598: `task finish` adopts a branch the user pushed by hand. A branch that changes a
//! workflow is still never pushed by `finish` (decision 127), but once the user has read it and
//! pushed it to `origin` at exactly the task's reviewed tip, `finish` opens (or adopts) its PR,
//! posts the in-task review and records the PR, pushing nothing.

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{ListedPr, PrInfo, PrState};
use sbxm::task::finish::{Finished, finish};
use sbxm::task::pipeline::Prepared;
use sbxm::task::record::{self, Process, Stage, Status};
use sbxm::task::risk::Level;

const WORKFLOW: &str = ".github/workflows/ci.yml";

const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done, `ready`, with the worker's commit of `files`.
fn ready_with(f: &Fixture, files: &[&str]) -> Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: files.iter().map(|s| (*s).to_owned()).collect(),
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
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    prepared
}

fn repo_git(prepared: &Prepared) -> std::path::PathBuf {
    prepared.meta.join("repo.git")
}

fn tip(prepared: &Prepared) -> String {
    git(&repo_git(prepared), &["rev-parse", "issue-41"])
}

/// Records an in-task review of `commit` with no must-fix findings, as the review would have.
fn reviewed_at(prepared: &mut Prepared, commit: &str) {
    prepared.record.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(commit.to_owned()),
        posted: None,
        must_fix: 0,
        full: true,
        repeat: false,
        risk: Level::Low,
        risk_reasons: Vec::new(),
        not_run: None,
    });
    record::write(&prepared.meta, &prepared.record).unwrap();
    // As the reviewer writes it: a `Scope:` line from the base to the reviewed commit.
    let base = git(&repo_git(prepared), &["rev-parse", "main"]);
    let review = REVIEW.replace(
        "Must-fix findings: 0\n",
        &format!("Must-fix findings: 0\n\nScope: `{base}..{commit}`\n"),
    );
    fs::write(prepared.meta.join("review.md"), review).unwrap();
}

/// A ready task whose worker committed `files`, reviewed at its tip.
fn reviewed_with(f: &Fixture, files: &[&str]) -> (Prepared, String) {
    let mut prepared = ready_with(f, files);
    let commit = tip(&prepared);
    reviewed_at(&mut prepared, &commit);
    (prepared, commit)
}

/// What the user does by hand: pushes `commit` from the task's repo.git as `issue-41` on origin.
fn push_by_hand(prepared: &Prepared, commit: &str) {
    git(
        &repo_git(prepared),
        &[
            "push",
            "--quiet",
            "--force",
            "origin",
            &format!("{commit}:refs/heads/issue-41"),
        ],
    );
}

/// From now on any push from the task's repo.git fails, while reading the remote still works:
/// a finish that pushes anything, even an empty update, is caught.
fn forbid_pushes(prepared: &Prepared) {
    git(
        &repo_git(prepared),
        &["config", "remote.origin.receivepack", "false"],
    );
}

/// A new commit in repo.git on top of `parent`, not on any branch.
fn commit_on(prepared: &Prepared, parent: &str) -> String {
    git(
        &repo_git(prepared),
        &[
            "commit-tree",
            &format!("{parent}^{{tree}}"),
            "-p",
            parent,
            "-m",
            "by hand",
        ],
    )
}

fn origin_head(f: &Fixture) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(&f.origin)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/issue-41"])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn reread(f: &Fixture) -> record::Record {
    Prepared::open(&f.env.base_dir(), "issue-41")
        .unwrap()
        .record
}

fn task_json(f: &Fixture) -> String {
    fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
}

fn run(f: &Fixture, github: &FakeGitHub) -> anyhow::Result<Finished> {
    finish(&f.env.base_dir(), "issue-41", github, &Probe)
}

/// The calls that change something on GitHub.
fn writes(github: &FakeGitHub) -> Vec<GhCall> {
    github
        .calls()
        .into_iter()
        .filter(|c| {
            !matches!(
                c,
                GhCall::OpenPrFor(..) | GhCall::IssueComment(..) | GhCall::IssueCommentEdit(..)
            )
        })
        .collect()
}

fn listed(number: u32) -> ListedPr {
    ListedPr {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        head_ref: "issue-41".into(),
        base_ref: "main".into(),
        is_cross_repository: false,
    }
}

/// The review comment and the `sbxm/review` status were posted on PR `pr` for `commit`.
fn assert_review_posted(github: &FakeGitHub, pr: u32, commit: &str) {
    let calls = writes(github);
    assert!(
        calls.iter().any(
            |c| matches!(c, GhCall::PrComment(_, n, body) if *n == pr && body.contains(commit))
        ),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, GhCall::CommitStatus(_, sha, _) if sha == commit)),
        "{calls:?}"
    );
}

// ---- A workflow change ----

#[test]
fn a_workflow_change_not_on_origin_is_refused_as_before() {
    let f = fixture();
    let (_prepared, _) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    let before = task_json(&f);
    let github = FakeGitHub::default();

    let message = format!("{:#}", run(&f, &github).unwrap_err());

    assert!(message.contains(WORKFLOW), "{message}");
    assert!(message.contains("secrets"), "{message}");
    assert_eq!(origin_head(&f), None, "nothing may be pushed");
    assert!(writes(&github).is_empty(), "{:?}", github.calls());
    assert_eq!(task_json(&f), before);
}

#[test]
fn a_workflow_change_pushed_by_hand_at_the_reviewed_tip_gets_its_pr_review_and_record() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);
    forbid_pushes(&prepared);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    assert_eq!(
        done.branch_adopted.as_deref(),
        Some(commit.as_str()),
        "{done:?}"
    );
    assert!(!done.adopted, "no open PR was there to adopt");
    assert_eq!(origin_head(&f).as_deref(), Some(commit.as_str()));
    let calls = writes(&github);
    let creates: Vec<_> = calls
        .iter()
        .filter(|c| matches!(c, GhCall::PrCreate(..)))
        .collect();
    assert_eq!(creates.len(), 1, "{calls:?}");
    let pr = sbxm::task::finish::pr_number(&done.url).unwrap();
    assert_review_posted(&github, pr, &commit);
    assert_eq!(done.review_posted.as_deref(), Some(commit.as_str()));
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
    assert_eq!(
        record.review.unwrap().posted.as_deref(),
        Some(commit.as_str())
    );
}

#[test]
fn a_workflow_change_pushed_by_hand_with_its_pr_open_adopts_that_pr() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);
    forbid_pushes(&prepared);
    let github = FakeGitHub::default().with_listed_pr("o/r", listed(551), PrState::Open);

    let done = run(&f, &github).unwrap();

    assert!(done.branch_adopted.is_some() && done.adopted, "{done:?}");
    assert_eq!(done.url, "https://github.com/o/r/pull/551");
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrCreate(..))),
        "no second PR: {:?}",
        github.calls()
    );
    assert_review_posted(&github, 551, &commit);
    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert_eq!(
        record.pr.as_deref(),
        Some("https://github.com/o/r/pull/551")
    );
}

/// The branch on origin is behind the task's tip (the base), ahead of it (a commit on top), or
/// diverged from it (a commit on the base): refused naming both commits, nothing pushed or
/// posted, the record unchanged.
#[test]
fn a_branch_on_origin_at_another_commit_is_refused_naming_both() {
    for workflow in [true, false] {
        for case in ["behind", "ahead", "diverged"] {
            let f = fixture();
            let files: &[&str] = if workflow {
                &["a.txt", WORKFLOW]
            } else {
                &["a.txt"]
            };
            let (prepared, commit) = reviewed_with(&f, files);
            let base = git(&repo_git(&prepared), &["rev-parse", "main"]);
            let other = match case {
                "behind" => base.clone(),
                "ahead" => commit_on(&prepared, &commit),
                _ => commit_on(&prepared, &base),
            };
            push_by_hand(&prepared, &other);
            let before = task_json(&f);
            let github = FakeGitHub::default();

            let message = format!("{:#}", run(&f, &github).unwrap_err());

            let what = format!("{case}, workflow {workflow}");
            assert!(message.contains(&other), "{what}: {message}");
            assert!(message.contains(&commit), "{what}: {message}");
            assert_eq!(origin_head(&f).as_deref(), Some(other.as_str()), "{what}");
            assert!(writes(&github).is_empty(), "{what}: {:?}", github.calls());
            assert_eq!(task_json(&f), before, "{what}");
        }
    }
}

/// The files a task's worker commits: with a workflow change, and without.
const BOTH: [&[&str]; 2] = [&["a.txt", WORKFLOW], &["a.txt"]];

/// With or without a workflow change, a branch at the tip whose review covered an older commit
/// is not adopted: the review would not be of the PR's head.
#[test]
fn a_branch_at_the_tip_whose_review_covered_an_older_commit_is_refused() {
    for files in BOTH {
        let f = fixture();
        let mut prepared = ready_with(&f, files);
        let commit = tip(&prepared);
        let base = git(&repo_git(&prepared), &["rev-parse", "main"]);
        reviewed_at(&mut prepared, &base);
        push_by_hand(&prepared, &commit);
        let before = task_json(&f);
        let github = FakeGitHub::default();

        let message = format!("{:#}", run(&f, &github).unwrap_err());

        assert!(message.contains(&commit), "{files:?}: {message}");
        assert!(message.contains("task review --pr"), "{files:?}: {message}");
        assert!(
            writes(&github).is_empty(),
            "{files:?}: {:?}",
            github.calls()
        );
        assert_eq!(task_json(&f), before, "{files:?}");
    }
}

#[test]
fn a_branch_at_the_tip_with_no_reviewed_commit_is_refused() {
    for files in BOTH {
        let f = fixture();
        let prepared = ready_with(&f, files);
        push_by_hand(&prepared, &tip(&prepared));
        let before = task_json(&f);
        let github = FakeGitHub::default();

        let message = format!("{:#}", run(&f, &github).unwrap_err());

        assert!(message.contains("task review --pr"), "{files:?}: {message}");
        assert!(
            writes(&github).is_empty(),
            "{files:?}: {:?}",
            github.calls()
        );
        assert_eq!(task_json(&f), before, "{files:?}");
    }
}

// ---- A branch without a workflow change ----

#[test]
fn a_branch_already_on_origin_at_the_tip_is_adopted_without_a_push() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt"]);
    push_by_hand(&prepared, &commit);
    forbid_pushes(&prepared);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    assert_eq!(
        done.branch_adopted.as_deref(),
        Some(commit.as_str()),
        "{done:?}"
    );
    let pr = sbxm::task::finish::pr_number(&done.url).unwrap();
    assert_review_posted(&github, pr, &commit);
    assert_eq!(reread(&f).pr.as_deref(), Some(done.url.as_str()));
}

#[test]
fn a_branch_not_on_origin_is_pushed_and_not_called_adopted() {
    let f = fixture();
    let (_prepared, commit) = reviewed_with(&f, &["a.txt"]);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    assert_eq!(done.branch_adopted, None, "{done:?}");
    assert_eq!(origin_head(&f).as_deref(), Some(commit.as_str()));
}

/// A finish whose PR create failed after its own push left a note; the retry pushes nothing and
/// says the branch was already on origin, not that the user pushed it.
#[test]
fn a_retry_after_finishs_own_push_says_the_branch_was_already_on_origin() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt"]);
    let down = FakeGitHub::default().failing_pr_create("gh: HTTP 502", false);
    let message = format!("{:#}", run(&f, &down).unwrap_err());
    assert!(message.contains("pushed issue-41 but"), "{message}");
    forbid_pushes(&prepared);

    let out = finish_command(&f, &FakeGitHub::default());

    assert!(
        out.contains("issue-41: adopted the branch already on origin: issue-41 at "),
        "{out}"
    );
    assert!(out.contains(&commit[..12]), "{out}");
    assert!(!out.contains("you pushed"), "{out}");
    assert_eq!(reread(&f).stage, Stage::Finished);
}

/// Review S-1: an adoption whose PR create failed leaves the same note as finish's own push; its
/// failure and its retry still say nothing was pushed, with every push refused throughout.
#[test]
fn a_retry_after_a_failed_adoption_says_nothing_was_pushed() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);
    forbid_pushes(&prepared);
    let down = FakeGitHub::default().failing_pr_create("gh: HTTP 502", false);

    let message = format!("{:#}", run(&f, &down).unwrap_err());

    assert!(message.contains("nothing was pushed"), "{message}");
    assert!(!message.contains("pushed issue-41 but"), "{message}");

    let out = finish_command(&f, &FakeGitHub::default());

    assert!(out.contains("issue-41: adopted the branch"), "{out}");
    assert!(out.contains("pushing nothing"), "{out}");
    assert!(!out.contains("pushed issue-41 and"), "{out}");
    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert_eq!(
        record.review.unwrap().posted.as_deref(),
        Some(commit.as_str())
    );
}

/// The read of the remote works but the push fails: nothing is opened and the record stays.
#[test]
fn a_push_that_fails_after_the_remote_was_read_opens_nothing() {
    let f = fixture();
    let (prepared, _) = reviewed_with(&f, &["a.txt"]);
    forbid_pushes(&prepared);
    let before = task_json(&f);
    let github = FakeGitHub::default();

    let message = format!("{:#}", run(&f, &github).unwrap_err());

    assert!(message.contains("cannot push issue-41"), "{message}");
    assert_eq!(origin_head(&f), None);
    assert!(writes(&github).is_empty(), "{:?}", github.calls());
    assert_eq!(task_json(&f), before);
}

// ---- After adoption ----

#[test]
fn an_adopted_task_already_finished_is_refused_or_only_retries_its_summaries() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);
    run(&f, &FakeGitHub::default()).unwrap();
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    // Issue 430: a finished task with its PR only posts its summaries again.
    assert!(done.summaries_only, "{done:?}");
    assert!(writes(&github).is_empty(), "{:?}", github.calls());

    // A finished task with no PR recorded is refused, as before.
    let mut record = reread(&f);
    record.pr = None;
    record::write(&prepared.meta, &record).unwrap();
    let message = format!("{:#}", run(&f, &FakeGitHub::default()).unwrap_err());
    assert!(message.contains("already finished"), "{message}");
}

#[test]
fn after_adoption_the_loop_counts_the_pr_and_rm_merged_removes_the_task() {
    let f = fixture();
    let (mut prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    // The loop marked the task as its own before its finish was refused (`Loop::claim`).
    prepared.record.hooks = serde_json::json!({ sbxm::task::queue::LOOP_HOOK: true });
    record::write(&prepared.meta, &prepared.record).unwrap();
    push_by_hand(&prepared, &commit);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    let pr = sbxm::task::finish::pr_number(&done.url).unwrap();
    let counted = sbxm::task::queue::loop_prs(&f.env.base_dir(), "o/r").unwrap();
    assert_eq!(
        counted.iter().map(|p| p.number).collect::<Vec<_>>(),
        vec![Some(pr)]
    );
    let github = github.with_pr(PrInfo {
        number: pr,
        head_ref: "issue-41".into(),
        head_oid: commit.clone(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });
    github.set_merged(pr, &commit);
    let plan = sbxm::task::merged::plan(&f.env.base_dir(), &github, &Probe).unwrap();
    assert!(plan.listing().contains("issue-41"), "{}", plan.listing());
}

// ---- What the command says ----

fn finish_command(f: &Fixture, github: &FakeGitHub) -> String {
    let mut out = Vec::new();
    sbxm::commands::task_finish::run(
        &f.env.config_dir(),
        &sbxm::commands::task_finish::Options {
            target: sbxm::commands::task_finish::Target::Issue(41),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        github,
        &Probe,
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn the_command_says_it_adopted_the_branch_and_pushed_nothing() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);

    let out = finish_command(&f, &FakeGitHub::default());

    assert!(
        out.contains("issue-41: adopted the branch you pushed: issue-41 at "),
        "{out}"
    );
    assert!(out.contains(&commit[..12]), "{out}");
    assert!(out.contains("opened https://github.com/o/r/pull/"), "{out}");
    assert!(!out.contains("pushed issue-41 and"), "{out}");
}

#[test]
fn the_command_says_it_adopted_the_branch_and_its_open_pr() {
    let f = fixture();
    let (prepared, commit) = reviewed_with(&f, &["a.txt", WORKFLOW]);
    push_by_hand(&prepared, &commit);
    let github = FakeGitHub::default().with_listed_pr("o/r", listed(551), PrState::Open);

    let out = finish_command(&f, &github);

    assert!(
        out.contains("issue-41: adopted the branch you pushed: issue-41 at "),
        "{out}"
    );
    assert!(
        out.contains("found https://github.com/o/r/pull/551 already open for it"),
        "{out}"
    );
}

// ---- A retry after a failed adoption (review M-1 of PR #610) ----

/// Moves the task's branch in repo.git to a new commit on top of `parent` and pushes it by hand,
/// past [`forbid_pushes`], which only stops finish's own pushes.
fn publish_newer_tip(prepared: &Prepared, parent: &str) -> String {
    let newer = commit_on(prepared, parent);
    git(
        &repo_git(prepared),
        &["update-ref", "refs/heads/issue-41", &newer],
    );
    git(
        &repo_git(prepared),
        &[
            "push",
            "--receive-pack=git-receive-pack",
            "--quiet",
            "--force",
            "origin",
            &format!("{newer}:refs/heads/issue-41"),
        ],
    );
    newer
}

/// The adoption of the reviewed tip A failed to open its PR; then a newer tip B was published.
/// The retry refuses B, whose review covers only A, before any call or write.
#[test]
fn a_retry_after_a_failed_adoption_refuses_a_newer_unreviewed_tip() {
    for files in BOTH {
        let f = fixture();
        let (prepared, reviewed) = reviewed_with(&f, files);
        push_by_hand(&prepared, &reviewed);
        forbid_pushes(&prepared);
        let down = FakeGitHub::default().failing_pr_create("gh: HTTP 502", false);
        run(&f, &down).unwrap_err();
        let newer = publish_newer_tip(&prepared, &reviewed);
        assert_eq!(origin_head(&f).as_deref(), Some(newer.as_str()));
        let before = task_json(&f);
        let github = FakeGitHub::default();

        let message = format!("{:#}", run(&f, &github).unwrap_err());

        assert!(message.contains(&newer), "{files:?}: {message}");
        assert!(message.contains(&reviewed), "{files:?}: {message}");
        assert!(message.contains("task review --pr"), "{files:?}: {message}");
        assert!(github.calls().is_empty(), "{files:?}: {:?}", github.calls());
        assert_eq!(task_json(&f), before, "{files:?}");
        assert_eq!(reread(&f).stage, Stage::Ready, "{files:?}");
    }
}

/// A workflow change is never pushed by finish, so even a record carrying the note of finish's
/// own push doesn't exempt an unreviewed tip of one from the reviewed-tip check.
#[test]
fn a_workflow_change_at_an_unreviewed_tip_is_refused_even_with_the_pushed_note() {
    let f = fixture();
    let mut prepared = ready_with(&f, &["a.txt", WORKFLOW]);
    let commit = tip(&prepared);
    let base = git(&repo_git(&prepared), &["rev-parse", "main"]);
    reviewed_at(&mut prepared, &base);
    prepared.record.notes.push(
        "the branch is pushed but its PR isn't open; run `sbxm task finish` again".to_owned(),
    );
    record::write(&prepared.meta, &prepared.record).unwrap();
    push_by_hand(&prepared, &commit);
    let before = task_json(&f);
    let github = FakeGitHub::default();

    let message = format!("{:#}", run(&f, &github).unwrap_err());

    assert!(message.contains(&commit), "{message}");
    assert!(message.contains("task review --pr"), "{message}");
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert_eq!(task_json(&f), before);
}

/// A retry at the unchanged, reviewed tip still completes, pushing nothing, and says the branch
/// was adopted as pushed by hand, since no finish pushed it.
#[test]
fn a_retry_after_a_failed_adoption_at_the_reviewed_tip_completes() {
    for files in BOTH {
        let f = fixture();
        let (prepared, commit) = reviewed_with(&f, files);
        push_by_hand(&prepared, &commit);
        forbid_pushes(&prepared);
        let down = FakeGitHub::default().failing_pr_create("gh: HTTP 502", false);
        run(&f, &down).unwrap_err();
        assert_eq!(reread(&f).notes.len(), 1, "{files:?}");

        let out = finish_command(&f, &FakeGitHub::default());

        assert!(
            out.contains("issue-41: adopted the branch you pushed: issue-41 at "),
            "{files:?}: {out}"
        );
        assert!(out.contains("pushing nothing"), "{files:?}: {out}");
        let record = reread(&f);
        assert_eq!(record.stage, Stage::Finished, "{files:?}");
        assert!(record.notes.is_empty(), "{files:?}: {:?}", record.notes);
        assert_eq!(
            record.review.unwrap().posted.as_deref(),
            Some(commit.as_str()),
            "{files:?}"
        );
    }
}
