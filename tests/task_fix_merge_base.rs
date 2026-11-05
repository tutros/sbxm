//! Issue 404, decision 292: `sbxm task fix --pr N` merges the base in the worker's sandbox before
//! it fixes the findings, resolves, gates, pushes, and its in-task review covers the merge
//! narrowly. One test per boundary case of the issue, plus the rules that avoid conflicts.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Probe, fixture_with, is_bundle, is_headless, ok, play_reviews,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_fix::{Options, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::config::Prompts;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::prompts::{self, Role};
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

/// The user's sbxm review of `head` with one must-fix finding.
fn review_of(head: &str) -> PrComment {
    PrComment {
        author: "tutros".into(),
        body: pr_comment(
            7,
            "",
            &format!(
                "Must-fix findings: 1\nRisk: medium\n\nScope: `0000000..{head}`\nIssues: pending\n\n\
                 ## Must fix\n\n### M-1 - The lock is never released\n\n\
                 **Where:** `src/lock.rs:10`\n**What happens:** it stays held\n\
                 **Why it matters:** decision 1\n**Fix:** release it\n"
            ),
        ),
    }
}

fn github(head: &str) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(pr7(head))
        .with_pr_comments(7, vec![review_of(head)])
}

const CLEAN: &str = "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing.\n";

/// A scratch clone of the stand-in origin at `refname`, for making commits to push.
fn scratch(f: &Fixture, name: &str, refname: &str) -> std::path::PathBuf {
    let dir = f.env.tmp.path().join(name);
    git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            dir.to_str().unwrap(),
        ],
    );
    git(&dir, &["checkout", "-q", "-B", "work", refname]);
    dir
}

/// Commits `file` with `text` appended on top of `from` and pushes it to `to` on the origin.
fn append_and_push(
    f: &Fixture,
    name: &str,
    from: &str,
    file: &str,
    text: &str,
    to: &[&str],
) -> String {
    let dir = scratch(f, name, from);
    let path = dir.join(file);
    let mut contents = fs::read_to_string(&path).unwrap_or_default();
    contents.push_str(text);
    fs::write(&path, contents).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", name]);
    for refname in to {
        git(
            &dir,
            &[
                "push",
                "-q",
                "-f",
                f.origin.to_str().unwrap(),
                &format!("HEAD:{refname}"),
            ],
        );
    }
    git(&dir, &["rev-parse", "HEAD"])
}

/// PR 7's head: `docs.md` gets a line on top of `main`, which also has `docs.md`.
fn pr_head(f: &Fixture) -> String {
    append_and_push(
        f,
        "docs-base",
        "origin/main",
        "docs.md",
        "# Docs\n",
        &["refs/heads/main"],
    );
    append_and_push(
        f,
        "pr-head",
        "origin/main",
        "docs.md",
        "- the PR's line\n",
        &["refs/heads/feature-x", "refs/pull/7/head"],
    )
}

/// `main` moves by a commit that appends `text` to `file`; returns its tip.
fn move_main(f: &Fixture, name: &str, file: &str, text: &str) -> String {
    append_and_push(f, name, "origin/main", file, text, &["refs/heads/main"])
}

fn origin_tip(f: &Fixture, refname: &str) -> String {
    git(&f.origin, &["rev-parse", refname])
}

fn is_ancestor(repo: &Path, ancestor: &str, tip: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, tip])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

/// What the played worker does on each headless run: merge `origin/main` first (unless `merge`
/// is off), keep both sides of a `docs.md` conflict, then commit its fix of M-1.
fn worker(
    workspace: &Path,
    merge: bool,
) -> impl Fn(&str, &sbxm::backend::ExecSpec) + Send + Sync + use<> {
    let workspace = workspace.to_path_buf();
    let runs = AtomicUsize::new(0);
    move |sandbox, spec| {
        if sandbox.contains("-review-") {
            return;
        }
        if is_headless(spec) {
            let run = runs.fetch_add(1, Ordering::SeqCst);
            if merge {
                let merged = Command::new("git")
                    .args(["merge", "--no-edit", "-q", "origin/main"])
                    .current_dir(&workspace)
                    .output()
                    .unwrap();
                if !merged.status.success() {
                    // The conflict at the end of docs.md: both sides stay, one after the other.
                    let text = fs::read_to_string(workspace.join("docs.md")).unwrap();
                    let resolved: String = text
                        .lines()
                        .filter(|l| {
                            !l.starts_with("<<<<<<<")
                                && !l.starts_with("=======")
                                && !l.starts_with(">>>>>>>")
                        })
                        .map(|l| format!("{l}\n"))
                        .collect();
                    fs::write(workspace.join("docs.md"), resolved).unwrap();
                    git(&workspace, &["add", "docs.md"]);
                    git(&workspace, &["commit", "-q", "--no-edit"]);
                }
            }
            fs::create_dir_all(workspace.join("src")).unwrap();
            fs::write(
                workspace.join("src/lock.rs"),
                format!("released {run} by {}\n", workspace.display()),
            )
            .unwrap();
            git(&workspace, &["add", "-A"]);
            git(&workspace, &["commit", "-q", "-m", "Release the lock"]);
            let dir = workspace.join(".sbxm-task");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("result.md"), "M-1 fixed\n").unwrap();
        } else if is_bundle(spec) {
            git(
                &workspace,
                &[
                    "bundle",
                    "create",
                    ".sbxm-task/branch.bundle",
                    "feature-x",
                    "^origin/main",
                ],
            );
        }
    }
}

/// The backend of a fix of PR 7 at `head`: the played worker, a clean review, and `cargo test`
/// failing the first `failing_gates` times it runs in the worker's sandbox.
fn backend(f: &Fixture, head: &str, merge: bool, failing_gates: usize) -> FakeBackend {
    let id = record::fix_id(7, head).unwrap();
    let worker = worker(&f.env.base_dir().join("tasks").join(&id), merge);
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN.to_owned()]);
    let gates = Arc::new(AtomicUsize::new(0));
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_responder(move |sandbox, spec| {
            let gate =
                !sandbox.contains("-review-") && spec.argv.iter().any(|a| a.contains("cargo test"));
            (gate && gates.fetch_add(1, Ordering::SeqCst) < failing_gates).then(|| ExecOutput {
                stdout: String::new(),
                stderr: "test docs_list ... FAILED\n".into(),
                exit_code: Some(101),
            })
        })
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        })
}

fn fix(f: &Fixture, backend: &FakeBackend, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
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
        },
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

fn task_of(f: &Fixture, head: &str) -> record::Record {
    let id = record::fix_id(7, head).unwrap();
    record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap()
}

#[test]
fn the_fix_prompts_merge_the_base_first_and_the_review_covers_the_merge_narrowly() {
    // A Windows checkout gives the prompt files CRLF line endings.
    let worker = prompts::template(Role::WorkerPrFix, &Prompts::default())
        .unwrap()
        .text
        .replace("\r\n", "\n");
    for part in [
        "First merge the base, before the findings: run `git merge --no-edit origin/{{base}}`",
        "keeping both\nsides whole",
        "a line both sides edited (a README row) gets both edits, word by word",
        "Never rebase, reset or force",
        "merge --abort` and hand it back under \"Needs a decision\"",
    ] {
        assert!(worker.contains(part), "{part}");
    }
    let reviewer = prompts::template(Role::ReviewerPrFix, &Prompts::default())
        .unwrap()
        .text
        .replace("\r\n", "\n");
    for part in [
        "`git log --merges {{scope_base}}..HEAD`",
        "the commits that merge brings in from {{base}} are\nnot this change",
        "`git show --remerge-diff <merge>`",
    ] {
        assert!(reviewer.contains(part), "{part}");
    }
}

#[test]
fn every_worker_prompt_keeps_new_tests_and_decisions_out_of_shared_ends() {
    for role in [Role::Worker, Role::WorkerSpec, Role::WorkerPrFix] {
        // A Windows checkout gives the prompt files CRLF line endings.
        let text = prompts::template(role, &Prompts::default())
            .unwrap()
            .text
            .replace("\r\n", "\n");
        for part in [
            "New tests go in a file of this task's own",
            "never appended at the end of a shared test\nfile",
            "(\"this adds decision N\")",
        ] {
            assert!(text.contains(part), "{role:?}: {part}");
        }
    }
    let skill = include_str!("../.claude/skills/sdlc-issue-flow/SKILL.md").replace("\r\n", "\n");
    for part in [
        "Decision 292: reserve the number of any decision the issue adds now",
        "never appended at the end of a shared\n  test file",
    ] {
        assert!(skill.contains(part), "{part}");
    }
}

#[test]
fn a_clean_merge_is_pushed_with_the_fix_and_needs_no_fix_round() {
    let f = config();
    let head = pr_head(&f);
    let main = move_main(&f, "main-other", "other.md", "unrelated\n");
    let backend = backend(&f, &head, true, 0);

    let (result, out) = fix(&f, &backend, &github(&head));

    result.unwrap();
    let tip = origin_tip(&f, "refs/heads/feature-x");
    assert!(is_ancestor(&f.origin, &head, &tip));
    assert!(is_ancestor(&f.origin, &main, &tip));
    let task = task_of(&f, &head);
    assert_eq!(task.round, 0, "no fix round");
    assert!(out.contains("pushed"), "{out}");
    // The worker was told to merge main, and the reviewer to review that merge narrowly.
    let meta = record::task_dir(&f.env.base_dir(), &task.id);
    let prompt = fs::read_to_string(meta.join("worker-prompt.md")).unwrap();
    assert!(
        prompt.contains("git merge --no-edit origin/main"),
        "{prompt}"
    );
    let review = fs::read_to_string(meta.join("reviewer-prompt-1.md")).unwrap();
    assert!(
        review.contains("git show --remerge-diff <merge>"),
        "{review}"
    );
}

#[test]
fn a_conflict_only_in_docs_is_resolved_keeping_both_sides_and_pushed() {
    let f = config();
    let head = pr_head(&f);
    move_main(&f, "main-docs", "docs.md", "- main's line\n");
    let backend = backend(&f, &head, true, 0);

    let (result, _) = fix(&f, &backend, &github(&head));

    result.unwrap();
    let tip = origin_tip(&f, "refs/heads/feature-x");
    let docs = git(&f.origin, &["show", &format!("{tip}:docs.md")]);
    assert!(
        docs.contains("- the PR's line") && docs.contains("- main's line"),
        "{docs}"
    );
    assert_eq!(task_of(&f, &head).round, 0);
}

#[test]
fn a_resolution_that_breaks_a_test_is_caught_by_the_gates_and_gets_a_fix_round() {
    let f = config();
    let head = pr_head(&f);
    let main = move_main(&f, "main-docs", "docs.md", "- main's line\n");
    let backend = backend(&f, &head, true, 1);

    let (result, _) = fix(&f, &backend, &github(&head));

    result.unwrap();
    let task = task_of(&f, &head);
    assert_eq!(task.round, 1, "one fix round after the failed gate");
    assert!(
        task.gates
            .iter()
            .any(|g| g.command == "cargo test" && g.passed),
        "{:?}",
        task.gates
    );
    let tip = origin_tip(&f, "refs/heads/feature-x");
    assert!(is_ancestor(&f.origin, &main, &tip));
}

#[test]
fn a_pr_whose_base_moved_twice_is_merged_again_by_the_next_fix() {
    let f = config();
    let head = pr_head(&f);
    let first = move_main(&f, "main-1", "one.md", "first\n");
    let (result, _) = fix(&f, &backend(&f, &head, true, 0), &github(&head));
    result.unwrap();
    let after_first = origin_tip(&f, "refs/heads/feature-x");
    assert!(is_ancestor(&f.origin, &first, &after_first));
    // GitHub moves the PR's head ref with its branch.
    git(&f.origin, &["update-ref", "refs/pull/7/head", &after_first]);
    let second = move_main(&f, "main-2", "two.md", "second\n");

    let (result, _) = fix(
        &f,
        &backend(&f, &after_first, true, 0),
        &github(&after_first),
    );

    result.unwrap();
    let tip = origin_tip(&f, "refs/heads/feature-x");
    assert!(is_ancestor(&f.origin, &after_first, &tip));
    assert!(is_ancestor(&f.origin, &second, &tip));
}

#[test]
fn a_worker_that_did_not_merge_the_base_pushes_nothing() {
    let f = config();
    let head = pr_head(&f);
    let main = move_main(&f, "main-other", "other.md", "unrelated\n");
    let backend = backend(&f, &head, false, 0);

    let (result, _) = fix(&f, &backend, &github(&head));

    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.contains(&format!("didn't merge main (at {main}) into feature-x")),
        "{err}"
    );
    assert!(
        err.contains("nothing was pushed to PR #7 (decision 292)"),
        "{err}"
    );
    assert_eq!(origin_tip(&f, "refs/heads/feature-x"), head);
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name.contains("-review-"))
    );
}

/// A review of `head` with no findings at all, by the user.
fn clean_review_of(head: &str) -> PrComment {
    PrComment {
        author: "tutros".into(),
        body: pr_comment(
            7,
            "",
            &format!("Must-fix findings: 0\nRisk: low\n\nScope: `0000000..{head}`\n\nNothing.\n"),
        ),
    }
}

fn github_with_clean_review(head: &str, base_moved: bool) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(PrInfo {
            base_moved,
            ..pr7(head)
        })
        .with_pr_comments(7, vec![clean_review_of(head)])
}

/// Issue 404, answer 1: a PR whose latest review has no findings still gets its base merged when
/// GitHub reports it conflicting or behind.
#[test]
fn a_conflicting_pr_with_a_clean_review_gets_its_base_merged_and_pushed() {
    let f = config();
    let head = pr_head(&f);
    let main = move_main(&f, "main-docs", "docs.md", "- main's line\n");
    let backend = backend(&f, &head, true, 0);

    let (result, _) = fix(&f, &backend, &github_with_clean_review(&head, true));

    result.unwrap();
    let tip = origin_tip(&f, "refs/heads/feature-x");
    assert!(is_ancestor(&f.origin, &main, &tip));
    let docs = git(&f.origin, &["show", &format!("{tip}:docs.md")]);
    assert!(
        docs.contains("- the PR's line") && docs.contains("- main's line"),
        "{docs}"
    );
}

/// ...and a PR GitHub doesn't report as conflicting or behind is refused before anything is made.
#[test]
fn a_clean_pr_with_a_clean_review_is_refused_before_any_write() {
    let f = config();
    let head = pr_head(&f);
    move_main(&f, "main-docs", "docs.md", "- main's line\n");
    let backend = backend(&f, &head, true, 0);

    let (result, _) = fix(&f, &backend, &github_with_clean_review(&head, false));

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("nothing to fix"), "{err}");
    assert!(backend.creates().is_empty());
    let id = record::fix_id(7, &head).unwrap();
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
}
