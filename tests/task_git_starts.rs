//! Issue #444: sbxm's own task code starts fewer `git` processes by answering several lookups
//! with one start. These tests pin what the batched calls return.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::task::repo;

/// Plain `git` for building fixtures (not the code under test).
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo_git: PathBuf,
}

/// A bare repo with `main` at one commit.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo_git = tmp.path().join("repo.git");
    fs::create_dir_all(&repo_git).unwrap();
    git(&repo_git, &["init", "-q", "--bare", "-b", "main"]);
    let tree = git(&repo_git, &["hash-object", "-t", "tree", "-w", "--stdin"]);
    let first = git(&repo_git, &["commit-tree", &tree, "-m", "first"]);
    git(&repo_git, &["update-ref", "refs/heads/main", &first]);
    Fixture {
        _tmp: tmp,
        repo_git,
    }
}

#[test]
fn creating_a_branch_returns_the_commit_it_starts_at() {
    let f = fixture();

    let start = repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    assert_eq!(start, git(&f.repo_git, &["rev-parse", "refs/heads/main"]));
    assert_eq!(start, repo::branch_tip(&f.repo_git, "issue-4").unwrap());
}

/// A commit object written straight to the repo (`hash-object` doesn't check what it names).
fn raw_commit(f: &Fixture, tree: &str, parent: &str) -> String {
    let body = format!(
        "tree {tree}\nparent {parent}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\nmsg\n"
    );
    let path = f._tmp.path().join("raw-commit");
    fs::write(&path, body).unwrap();
    git(
        &f.repo_git,
        &["hash-object", "-t", "commit", "-w", path.to_str().unwrap()],
    )
}

#[test]
fn a_rebuild_over_a_commit_whose_tree_is_missing_writes_nothing() {
    use sbxm::task::rewrite;
    let f = fixture();
    let base = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let missing_tree = "1".repeat(40);
    let agent = raw_commit(&f, &missing_tree, &base);
    git(
        &f.repo_git,
        &["update-ref", "refs/sbxm/agent/issue-4", &agent],
    );
    git(&f.repo_git, &["update-ref", "refs/heads/issue-4", &base]);
    let before = git(&f.repo_git, &["count-objects"]);

    let err = rewrite::rebuild(&f.repo_git, "main", "issue-4").unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("tree") && message.contains(&missing_tree),
        "{message}"
    );
    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]), base);
    assert_eq!(git(&f.repo_git, &["count-objects"]), before);
}
