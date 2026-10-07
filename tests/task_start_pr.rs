//! Decision 169 (d), issue #83: a task for an issue tied to an open PR (`PR: #m` on the first
//! line of its body) starts from that PR's branch head, fetched into the host-owned `repo.git`,
//! and records the PR, its branch and the head commit it started from. A closed, merged or fork
//! PR is refused before any write.

mod common;

use std::fs;
use std::path::PathBuf;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Probe, add_pr_head, backend, ctx, fixture, is_bundle, is_headless,
    issue_text, ok, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_status;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{IssueText, PrInfo, PrState};
use sbxm::task::pipeline;
use sbxm::task::record::{self, Kind, NewTask, PrBranch, Process};

fn pr(number: u32, state: PrState, fork: bool) -> PrInfo {
    PrInfo {
        number,
        head_ref: "feature-x".into(),
        is_cross_repository: fork,
        state,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
    }
}

/// Issue 41, a finding of PR `pr`: its body's first line names the PR.
fn finding(pr: u32) -> IssueText {
    IssueText {
        number: 41,
        title: "Fix 41".into(),
        state: "OPEN".into(),
        text: format!(
            "title:\tFix 41\nstate:\tOPEN\nlabels:\tmust-fix\n--\nPR: #{pr}\n\n**Acceptance criteria:** do it\n"
        ),
    }
}

fn nothing_new(f: &Fixture) -> bool {
    let empty_or_absent =
        |dir: PathBuf| fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none());
    empty_or_absent(f.env.base_dir().join("tasks"))
        && empty_or_absent(f.env.base_dir().join(".sbxm").join("tasks"))
}

#[test]
fn an_issue_of_an_open_pr_starts_from_the_prs_branch_head_and_records_it() {
    let f = fixture();
    let sha = add_pr_head(&f, 7);
    let backend = backend();
    let github = FakeGitHub::default().with_pr(pr(7, PrState::Open, false));
    let source = source(&f);

    let prepared = pipeline::prepare(&ctx(&f, &source, &backend, &github), &finding(7)).unwrap();

    assert!(
        github.calls().contains(&GhCall::Pr("o/r".into(), 7)),
        "{:?}",
        github.calls()
    );
    // The PR's head, fetched into repo.git under the PR's own branch name.
    let repo_git = prepared.meta.join("repo.git");
    assert_eq!(git(&repo_git, &["rev-parse", "refs/heads/feature-x"]), sha);
    // The worker's clone is on that branch, at that commit, with the PR's change in it.
    let ws = &prepared.workspace;
    assert_eq!(git(ws, &["branch", "--show-current"]), "feature-x");
    assert_eq!(git(ws, &["rev-parse", "HEAD"]), sha);
    assert!(ws.join("pr.txt").is_file());

    let record = record::read(&prepared.meta.join("task.json")).unwrap();
    assert_eq!(record.id, "issue-41");
    assert_eq!(record.branch, "feature-x");
    assert_eq!(
        record.continues,
        Some(PrBranch {
            pr: 7,
            branch: "feature-x".into(),
            base: sha.clone(),
        })
    );
    let json = fs::read_to_string(prepared.meta.join("task.json")).unwrap();
    assert!(
        json.contains("\"continues\"") && json.contains(&sha),
        "{json}"
    );

    // Always a fresh sandbox of its own.
    let creates = backend.creates();
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0].name, "sbxm-task-issue-41-claude");
    assert_eq!(&creates[0].workspace, ws);
}

#[test]
fn the_worker_prompt_names_the_pr_and_says_the_branch_has_commits() {
    let f = fixture();
    add_pr_head(&f, 7);
    let backend = backend();
    let github = FakeGitHub::default().with_pr(pr(7, PrState::Open, false));
    let source = source(&f);

    let prepared = pipeline::prepare(&ctx(&f, &source, &backend, &github), &finding(7)).unwrap();

    let prompt =
        fs::read_to_string(prepared.workspace.join(".sbxm-task").join("prompt.md")).unwrap();
    assert!(prompt.contains("pull request #7"), "{prompt}");
    assert!(prompt.contains("feature-x"), "{prompt}");
    assert!(prompt.contains("already has commits"), "{prompt}");
}

#[test]
fn an_issue_without_a_pr_line_starts_from_the_base_as_before() {
    let f = fixture();
    let backend = backend();
    let github = FakeGitHub::default();
    let source = source(&f);

    let prepared =
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert_eq!(prepared.record.branch, "issue-41");
    assert_eq!(prepared.record.continues, None);
    let prompt =
        fs::read_to_string(prepared.workspace.join(".sbxm-task").join("prompt.md")).unwrap();
    assert!(!prompt.contains("pull request #"), "{prompt}");
}

#[test]
fn a_closed_merged_or_fork_pr_is_refused_before_any_write() {
    for (state, fork, word) in [
        (PrState::Closed, false, "closed"),
        (PrState::Merged, false, "merged"),
        (PrState::Open, true, "fork"),
    ] {
        let f = fixture();
        add_pr_head(&f, 7);
        let backend = backend();
        let github = FakeGitHub::default().with_pr(pr(7, state, fork));
        let source = source(&f);

        let err = pipeline::prepare(&ctx(&f, &source, &backend, &github), &finding(7))
            .unwrap_err()
            .to_string();

        assert!(err.contains("PR #7") && err.contains(word), "{err}");
        assert!(err.contains("#41"), "{err}");
        assert!(nothing_new(&f), "{word}: something was written");
        assert!(backend.creates().is_empty(), "{word}");
    }
}

#[test]
fn a_pr_that_cannot_be_read_is_refused_before_any_write() {
    let f = fixture();
    let backend = backend();
    let github = FakeGitHub::default(); // PR 7 isn't scripted
    let source = source(&f);

    let err = pipeline::prepare(&ctx(&f, &source, &backend, &github), &finding(7))
        .unwrap_err()
        .to_string();

    assert!(err.contains("PR #7"), "{err}");
    assert!(nothing_new(&f));
    assert!(backend.creates().is_empty());
}

#[test]
fn a_record_without_continues_still_loads() {
    let f = fixture();
    let dir = record::task_dir(&f.env.base_dir(), "issue-5");
    let task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 5,
            repo: "o/r",
            title: "Old",
            base: "main",
            branch: "issue-5",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 0),
    );
    record::write(&dir, &task).unwrap();
    let json = fs::read_to_string(dir.join("task.json")).unwrap();
    assert!(!json.contains("continues"), "{json}");

    let read = record::read(&dir.join("task.json")).unwrap();
    assert_eq!(read.continues, None);
}

// ---- Review M-1: a continued task counts only the worker's commits, not the PR's ----

/// A backend whose worker commits `commits` files in issue 41's clone, and whose bundle command
/// runs the real `git bundle create` with the arguments the pipeline sent, answering as git does
/// (`Refusing to create empty bundle` when nothing is past the excluded commit).
fn bundling(f: &Fixture, commits: usize) -> FakeBackend {
    let ws = f.env.base_dir().join("tasks").join("issue-41");
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_responder(move |_, spec| {
            if is_headless(spec) {
                for n in 0..commits {
                    let file = format!("worker-{n}.txt");
                    fs::write(ws.join(&file), "x").unwrap();
                    git(&ws, &["add", "-A"]);
                    git(&ws, &["commit", "-q", "-m", &file]);
                }
                None
            } else if is_bundle(spec) {
                fs::create_dir_all(ws.join(".sbxm-task")).unwrap();
                // argv: git -C <ws> bundle create <file> <refs...>
                let out = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&ws)
                    .args(&spec.argv[3..])
                    .output()
                    .unwrap();
                Some(ExecOutput {
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    exit_code: out.status.code(),
                })
            } else {
                None
            }
        })
}

fn ahead_shown(f: &Fixture) -> String {
    let (text, _ids) = task_status::render(&f.env.base_dir(), None, false, &Probe).unwrap();
    text.split("ahead:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_continued_task_whose_worker_adds_nothing_counts_zero_commits() {
    let f = fixture();
    add_pr_head(&f, 7);
    let backend = bundling(&f, 0);
    let github = FakeGitHub::default().with_pr(pr(7, PrState::Open, false));
    let source = source(&f);
    let ctx = ctx(&f, &source, &backend, &github);

    let mut prepared = pipeline::prepare(&ctx, &finding(7)).unwrap();
    let worked = pipeline::run_worker(&ctx, &mut prepared).unwrap();

    assert_eq!(worked.commits, 0, "the PR's own commit isn't the worker's");
    assert!(
        worked.notes.iter().any(|n| n.contains("no commits")),
        "{:?}",
        worked.notes
    );
    assert_eq!(ahead_shown(&f), "0");
}

#[test]
fn a_continued_task_whose_worker_adds_one_commit_counts_one() {
    let f = fixture();
    add_pr_head(&f, 7);
    let backend = bundling(&f, 1);
    let github = FakeGitHub::default().with_pr(pr(7, PrState::Open, false));
    let source = source(&f);
    let ctx = ctx(&f, &source, &backend, &github);

    let mut prepared = pipeline::prepare(&ctx, &finding(7)).unwrap();
    let worked = pipeline::run_worker(&ctx, &mut prepared).unwrap();

    assert_eq!(worked.commits, 1);
    assert_eq!(ahead_shown(&f), "1");
    // The PR's commit and the worker's are both in repo.git's copy of the branch.
    let repo_git = prepared.meta.join("repo.git");
    assert_eq!(
        git(&repo_git, &["rev-list", "--count", "main..feature-x"]),
        "2"
    );
}
