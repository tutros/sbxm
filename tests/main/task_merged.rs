//! Issue 202, decision 195: `task::merged::plan` finds the tasks whose work is merged.

use crate::common;

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

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
        head_repo: String::new(),
        state: PrState::Open,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

fn open_pr(number: u32, head_oid: String) -> PrInfo {
    PrInfo {
        number,
        head_ref: "feature-x".into(),
        head_oid,
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
        base_moved: false,
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
    let mut prepared = pipeline::prepare_pr(&ctx(f, &source, &backend, &github), &pr).unwrap();
    // Its process is still working on it, or was cut off: not a run that ended (issue 597).
    prepared.lock = None;
    record::write(&prepared.meta, &prepared.record).unwrap();
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
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Gone).unwrap();

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
        head_repo: String::new(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Gone).unwrap();

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
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    });

    // Probe says the process is alive: the task looks like it's still running its review.
    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
        head_repo: String::new(),
        state: PrState::Closed,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
        &sbxm::backend::FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    (spec, id)
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
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    let cleanup = found.candidates[0]
        .local_cleanup
        .as_ref()
        .unwrap_or_else(|| panic!("{found:?}"));
    // Each printed command is a whole, runnable line (prose on lines of its own), and the branch
    // deletion is git's non-forcing one, which refuses a branch with commits HEAD doesn't have.
    let expected = [
        "git worktree remove <path>".to_owned(),
        format!("git branch -d {id}"),
    ];
    assert_eq!(cleanup, &expected);
    let listing = found.listing();
    let lines: Vec<&str> = listing.lines().map(str::trim).collect();
    for cmd in &expected {
        assert!(lines.contains(&cmd.as_str()), "{listing}");
    }
    assert!(!listing.contains("branch -D"), "{listing}");
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
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert!(found.candidates[0].local_cleanup.is_none(), "{found:?}");
}

#[test]
fn a_spec_task_with_no_commits_yet_is_not_a_candidate() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let backend = sbxm::backend::FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = FakeGitHub::default();
    let source = source(&f);
    pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

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
    merge_into_origin_main(&f, &id);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, id);
    assert!(
        found.candidates[0].reason.contains("merged"),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}

/// Issue 202's round-3 M-2: a spec task whose folder is a link is left alone, named, before its
/// `repo.git` is fetched into or read through the link.
#[test]
fn a_spec_task_whose_folder_is_a_link_is_left_alone_without_fetching_through_it() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let elsewhere = f.env.tmp.path().join("elsewhere-task");
    fs::rename(&meta, &elsewhere).unwrap();
    common::dir_link(&meta, &elsewhere);
    let fetch_head = elsewhere.join("repo.git").join("FETCH_HEAD");
    let before = fs::read(&fetch_head).ok();
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("link"),
        "{}",
        found.left_alone[0].reason
    );
    assert_eq!(
        fs::read(&fetch_head).ok(),
        before,
        "fetched through the link"
    );
}

/// Issue 202's round-4 M-1: a linked task folder is left alone before anything behind the link is
/// read, so an invalid `task.json` there neither surfaces nor stops the rest of the scan.
#[test]
fn a_linked_task_folder_is_left_alone_without_reading_its_record() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let elsewhere = f.env.tmp.path().join("elsewhere-task");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(
        elsewhere.join("task.json"),
        "this came from behind the link",
    )
    .unwrap();
    let linked = record::task_dir(&f.env.base_dir(), "issue-41");
    common::dir_link(&linked, &elsewhere);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    let ids: Vec<&str> = found.left_alone.iter().map(|l| l.id.as_str()).collect();
    assert!(ids.contains(&"issue-41"), "{found:?}");
    assert!(
        ids.contains(&id.as_str()),
        "the real task wasn't scanned: {found:?}"
    );
    let linked_reason = &found
        .left_alone
        .iter()
        .find(|l| l.id == "issue-41")
        .unwrap()
        .reason;
    assert!(linked_reason.contains("link"), "{linked_reason}");
}

/// Issue 202's round-4 M-1, the same kind one level down: a `task.json` that is itself a link is
/// left alone, named, and never read.
#[cfg(unix)]
#[test]
fn a_task_record_that_is_a_link_is_left_alone_without_reading_it() {
    let f = fixture();
    let elsewhere = f.env.tmp.path().join("elsewhere-task.json");
    fs::write(&elsewhere, "this came from behind the link").unwrap();
    let folder = record::task_dir(&f.env.base_dir(), "issue-41");
    fs::create_dir_all(&folder).unwrap();
    common::file_link(&folder.join("task.json"), &elsewhere);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, "issue-41");
    assert!(
        found.left_alone[0].reason.contains("symlink"),
        "{}",
        found.left_alone[0].reason
    );
}

/// Issue 202's round-4 M-1: if `.sbxm/tasks` itself is a link, the scan refuses before listing
/// or reading anything behind it.
#[test]
fn a_linked_tasks_root_refuses_the_scan() {
    let f = fixture();
    let elsewhere = f.env.tmp.path().join("elsewhere-tasks");
    fs::create_dir_all(elsewhere.join("issue-41")).unwrap();
    fs::write(
        elsewhere.join("issue-41").join("task.json"),
        "this came from behind the link",
    )
    .unwrap();
    let root = record::tasks_root(&f.env.base_dir());
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    let _ = fs::remove_dir_all(&root);
    common::dir_link(&root, &elsewhere);
    let github = FakeGitHub::default();

    let err = plan(&f.env.base_dir(), &github, &Probe).unwrap_err();

    assert!(format!("{err:#}").contains("link"), "{err:#}");
}

/// PR 241 review M-1: a spec task whose `repo.git` is a link is left alone, named, without
/// fetching into the repository behind the link (decision 159: `repo.git` is host-owned).
#[test]
fn a_linked_repo_git_is_left_alone_without_fetching_through_it() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    merge_into_origin_main(&f, &id);
    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    let elsewhere = f.env.tmp.path().join("elsewhere-repo.git");
    fs::rename(&repo_git, &elsewhere).unwrap();
    common::dir_link(&repo_git, &elsewhere);
    let fetch_head = elsewhere.join("FETCH_HEAD");
    let before = fs::read(&fetch_head).ok();
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("link"),
        "{}",
        found.left_alone[0].reason
    );
    assert_eq!(
        fs::read(&fetch_head).ok(),
        before,
        "fetched through the link"
    );
}

/// PR 241 review M-2: a `task.json` whose `id` isn't its own folder's name never selects the task
/// it names: its folder is left alone under the folder's name, and the named task, whose own PR
/// is open, is not a candidate.
#[test]
fn a_record_whose_id_is_not_its_folder_is_left_alone_and_selects_nothing() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let real = record::task_dir(&f.env.base_dir(), "issue-41").join("task.json");
    let other_url = format!("{}777", url.trim_end_matches(char::is_numeric));
    let forged = fs::read_to_string(&real).unwrap().replace(&url, &other_url);
    let stray = record::task_dir(&f.env.base_dir(), "issue-99");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("task.json"), forged).unwrap();
    let github = FakeGitHub::default()
        .with_pr(open_pr(number, "0".repeat(40)))
        .with_pr(PrInfo {
            number: 777,
            head_ref: "issue-41".into(),
            head_oid: "0".repeat(40),
            is_cross_repository: false,
            head_repo: String::new(),
            state: PrState::Merged,
            title: "Fix 41".into(),
            body: String::new(),
            closing_issues: vec![41],
            base_moved: false,
        });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, "issue-99");
    assert!(
        found.left_alone[0].reason.contains("issue-41"),
        "{}",
        found.left_alone[0].reason
    );
}

/// Issue 319: points `id`'s record at a base branch `base` that its `repo.git` has (at `main`'s
/// commit, as `--base <pr-branch>` would have cloned it) but that `origin` never had or has
/// since deleted, as GitHub does to a merged PR's head branch.
fn rebase_record_onto(f: &Fixture, id: &str, base: &str) -> std::path::PathBuf {
    let meta = record::task_dir(&f.env.base_dir(), id);
    let path = meta.join("task.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    json["base"] = serde_json::Value::String(base.to_owned());
    fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let repo_git = meta.join("repo.git");
    common::git(
        &repo_git,
        &["--git-dir", ".", "branch", base, "refs/heads/main"],
    );
    repo_git
}

#[test]
fn a_spec_task_whose_base_is_gone_on_origin_is_left_alone_with_its_removal_command() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    rebase_record_onto(&f, &id, "m4-plan");

    let found = plan(&f.env.base_dir(), &FakeGitHub::default(), &Probe).unwrap();

    assert!(found.candidates.is_empty(), "{found:?}");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    let reason = &found.left_alone[0].reason;
    assert!(reason.contains("m4-plan"), "{reason}");
    assert!(reason.contains("gone on origin"), "{reason}");
    assert!(reason.contains("sbxm task rm --spec"), "{reason}");
    let listing = found.listing();
    assert!(
        listing.contains(&format!("left alone, task {id} (idea.md): "))
            && listing.contains("gone on origin"),
        "{listing}"
    );
}

#[test]
fn a_gone_base_does_not_stop_the_scan_for_a_merged_task_next_to_it() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let (_spec, id) = ready_spec_task(&f);
    rebase_record_onto(&f, &id, "m4-plan");
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, "issue-41");
    assert_eq!(found.left_alone.len(), 1, "{found:?}");
    assert_eq!(found.left_alone[0].id, id);
    assert!(
        found.left_alone[0].reason.contains("gone on origin"),
        "{}",
        found.left_alone[0].reason
    );
}

#[test]
fn a_fetch_failing_while_the_base_is_still_on_origin_stops_the_scan() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    // A stale lock on the remote-tracking ref: the fetch can't update it, but `origin` still has
    // `main`, so `ls-remote` finds it.
    fs::create_dir_all(repo_git.join("refs/remotes/origin")).unwrap();
    fs::write(repo_git.join("refs/remotes/origin/main.lock"), "").unwrap();

    let err = plan(&f.env.base_dir(), &FakeGitHub::default(), &Probe).unwrap_err();

    let text = format!("{err:#}");
    assert!(text.contains("cannot fetch main from origin"), "{text}");
    assert!(text.contains(&id), "{text}");
}

#[test]
fn a_failing_ls_remote_after_a_failed_fetch_stops_the_scan() {
    let f = fixture();
    let (_spec, id) = ready_spec_task(&f);
    let repo_git = rebase_record_onto(&f, &id, "m4-plan");
    let nowhere = f.env.tmp.path().join("no-such-origin");
    common::git(
        &repo_git,
        &[
            "--git-dir",
            ".",
            "remote",
            "set-url",
            "origin",
            nowhere.to_str().unwrap(),
        ],
    );

    let err = plan(&f.env.base_dir(), &FakeGitHub::default(), &Probe).unwrap_err();

    let text = format!("{err:#}");
    assert!(text.contains(&id), "{text}");
    assert!(text.contains("cannot fetch m4-plan from origin"), "{text}");
    assert!(text.contains("still on origin"), "{text}");
}

/// Issue 313: a fix task's work is its PR's, so it goes once that PR is merged.
#[test]
fn a_fix_task_whose_pr_is_merged_is_a_candidate() {
    let f = fixture();
    let id = "fix-7-1234567890ab";
    let mut task = record::Record::new(
        &record::NewTask {
            kind: record::Kind::Fix,
            number: 7,
            repo: "o/r",
            title: "Add the x feature",
            base: "main",
            branch: "feature-x",
            config_hash: "h",
            id: Some(id),
        },
        0,
        Process::new(1, 0),
    );
    task.stage = Stage::Finished;
    task.status = Status::Ok;
    record::write(&record::task_dir(&f.env.base_dir(), id), &task).unwrap();
    let github = FakeGitHub::default().with_pr(PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Add the x feature".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    });

    let found = plan(&f.env.base_dir(), &github, &Gone).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, id);
    assert!(found.candidates[0].reason.contains("#7"));
}
