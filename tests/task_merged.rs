//! Issue 202, decision 195: `task::merged::plan` finds the tasks whose work is merged.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, add_pr_head, backend, ctx, fixture, issue_text, ok, play,
    ready_spec_task, source, spec_file,
};
use sbxm::commands::task_finish;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::finish::finish;
use sbxm::task::merged::plan;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Process, ProcessProbe, Stage, Status};

/// A task's `sbxm` process is gone (issue 202 tests): `task rm --merged` can plan a PR-review
/// task without driving it through a full review round first.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// As `review::with_header` saves it: the reviewer's line, a blank line, then the review.
const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done by hand: `ready`, with a result and a review.
fn ready(f: &Fixture) -> sbxm::task::pipeline::Prepared {
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
    let mut prepared = common::task_fixture::worked_task(f, &b);
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

/// `ready`, then finished (pushed and a PR opened): `task.pr` names the PR `finish` opened.
fn finished(f: &Fixture) -> String {
    ready(f);
    let github = FakeGitHub::default();
    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();
    done.url
}

fn pr_number(url: &str) -> u32 {
    sbxm::task::finish::pr_number(url).unwrap()
}

fn repo_root(f: &Fixture) -> std::path::PathBuf {
    f.env.tmp.path().join("target-repo")
}

#[test]
fn an_issue_task_with_no_pr_yet_is_not_a_candidate() {
    let f = fixture();
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
    common::task_fixture::worked_task(&f, &b);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn an_issue_task_whose_pr_is_merged_is_a_candidate() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, "issue-41");
    assert!(
        found.candidates[0].reason.contains(&format!("#{number}")),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn an_issue_task_whose_pr_is_open_is_left_alone() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

fn open_pr(number: u32, head_oid: String) -> PrInfo {
    PrInfo {
        number,
        head_ref: "feature-x".into(),
        head_oid,
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
    }
}

/// Prepares a PR-review task for PR `number` (open at the time, as `prepare_pr` requires).
fn pr_review_task(f: &Fixture, number: u32) {
    let backend = sbxm::backend::FakeBackend::with_secrets(&["anthropic", "openai"]);
    let head_oid = add_pr_head(f, number);
    let pr = open_pr(number, head_oid);
    let github = FakeGitHub::default()
        .with_pr(pr.clone())
        .with_issue_text(issue_text(4));
    let source = source(f);
    pipeline::prepare_pr(&ctx(f, &source, &backend, &github), &pr).unwrap();
}

#[test]
fn a_pr_review_task_whose_pr_is_merged_is_a_candidate() {
    let f = fixture();
    pr_review_task(&f, 7);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Gone).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, "pr-7");
    assert!(found.candidates[0].reason.contains("#7"));
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn a_pr_review_task_whose_pr_is_still_open_is_left_alone() {
    let f = fixture();
    pr_review_task(&f, 7);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Gone).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn a_running_pr_review_task_whose_pr_is_merged_is_left_alone() {
    let f = fixture();
    pr_review_task(&f, 7);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
    });

    // Probe says the process is alive: the task looks like it's still running its review.
    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, "pr-7");
}

#[test]
fn an_issue_task_whose_pr_closed_without_merging_is_left_alone() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Closed,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

/// A spec task finished through the `local` sink (decision 174(e)): its result lives only in its
/// own `repo.git` until fetched.
fn kept_spec_task(f: &Fixture) -> (std::path::PathBuf, String) {
    let (spec, id) = ready_spec_task(f);
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(spec.clone()),
            repo_root: repo_root(f),
            push_unresolved: false,
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    (spec, id)
}

/// A checkout of `f.origin` with `id`'s branch fetched out of its `repo.git` (decision 178's
/// protection satisfied).
fn fetched_checkout(f: &Fixture, id: &str) -> std::path::PathBuf {
    let checkout = f.env.tmp.path().join("checkout");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    let repo_git = record::task_dir(&f.env.base_dir(), id).join("repo.git");
    common::git(
        &checkout,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    checkout
}

/// Fast-forward merges `id`'s branch (from its `repo.git`) into `main` on `f.origin`, as a real
/// GitHub merge would land it.
fn merge_into_origin_main(f: &Fixture, id: &str) {
    let repo_git = record::task_dir(&f.env.base_dir(), id).join("repo.git");
    let scratch = f.env.tmp.path().join("merge-scratch");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    common::git(
        &scratch,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    common::git(&scratch, &["merge", "--ff-only", "-q", id]);
    common::git(&scratch, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_finished_spec_task_not_yet_merged_is_left_alone_as_not_merged() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    let checkout = fetched_checkout(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &checkout, &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("not merged"),
        "{}",
        found.left_alone[0].reason
    );
    assert!(
        found.left_alone[0].reason.contains("--spec"),
        "{}",
        found.left_alone[0].reason
    );
}

#[test]
fn a_merged_and_fetched_spec_task_is_a_candidate() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    let checkout = fetched_checkout(&f, &id);
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &checkout, &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, id);
    assert!(
        found.candidates[0].reason.contains("merged"),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}

/// Issue 202's M-1: `task rm --merged` never deletes a spec task's own worktree or local branch,
/// so it names the two git commands that do instead, for both the dry-run/confirmation listing
/// and (checked in `tests/task_merged_remove.rs`) the post-removal summary.
#[test]
fn a_merged_spec_candidate_names_its_worktree_and_branch_cleanup_commands() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    let checkout = fetched_checkout(&f, &id);
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &checkout, &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    let cleanup = found.candidates[0]
        .local_cleanup
        .as_ref()
        .unwrap_or_else(|| panic!("{found:?}"));
    assert!(cleanup[0].contains("git worktree remove"), "{cleanup:?}");
    assert!(
        cleanup[1].contains(&format!("git branch -D {id}")),
        "{cleanup:?}"
    );
    let listing = found.listing();
    assert!(listing.contains("git worktree remove"), "{listing}");
    assert!(
        listing.contains(&format!("git branch -D {id}")),
        "{listing}"
    );
}

/// An issue task's cleanup never mentions a worktree or local branch: there is nothing of the
/// kind for `task rm --merged` to leave alone.
#[test]
fn an_issue_candidate_has_no_local_cleanup() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert!(found.candidates[0].local_cleanup.is_none(), "{found:?}");
}

#[test]
fn a_merged_but_unfetched_spec_task_is_left_alone_protected() {
    let f = fixture();
    let (_spec, id) = kept_spec_task(&f);
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    // repo_root here is the sbxm-task.toml checkout, which never fetched the branch.
    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("git fetch"),
        "{}",
        found.left_alone[0].reason
    );
}

#[test]
fn a_spec_task_with_no_commits_yet_is_not_a_candidate() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let backend = sbxm::backend::FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = FakeGitHub::default();
    let source = source(&f);
    pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

/// Issue 202's M-3: a spec task doesn't need `task finish --spec` to have run before its branch's
/// mergedness is checked; only whether it has commits and isn't still running.
#[test]
fn a_ready_but_unfinished_spec_task_not_yet_merged_is_left_alone_as_not_merged() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("not merged"),
        "{}",
        found.left_alone[0].reason
    );
    assert!(
        found.left_alone[0].reason.contains("--spec"),
        "{}",
        found.left_alone[0].reason
    );
}

/// Issue 202's M-3: a `Ready` spec task (never `finish --spec`ed) whose branch is already
/// reachable from `origin/<base>` — because the user fetched and merged it by hand — is a
/// candidate, not silently kept.
#[test]
fn a_ready_but_unfinished_spec_task_whose_branch_is_merged_is_a_candidate() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let checkout = fetched_checkout(&f, &id);
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &checkout, &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, id);
    assert!(
        found.candidates[0].reason.contains("merged"),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}
