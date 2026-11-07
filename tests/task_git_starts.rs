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
        message.contains("tree") && message.contains(&agent),
        "{message}"
    );
    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]), base);
    assert_eq!(git(&f.repo_git, &["count-objects"]), before);
}

/// A workspace whose `.sbxm-task/branch.bundle` is a bundle of `refs` made in a clone of the
/// fixture's repo.
fn workspace_with_bundle(f: &Fixture, refs: &[&str]) -> PathBuf {
    let workspace = f._tmp.path().join("task").join("workspace");
    fs::create_dir_all(workspace.join(".sbxm-task")).unwrap();
    let bundle = workspace.join(".sbxm-task/branch.bundle");
    let mut args = vec!["bundle", "create", bundle.to_str().unwrap()];
    args.extend_from_slice(refs);
    git(&f.repo_git, &args);
    workspace
}

#[test]
fn a_bundle_without_the_task_branch_is_refused_by_name() {
    let f = fixture();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    let workspace = workspace_with_bundle(&f, &["refs/heads/main"]);

    let err = repo::fetch_bundle(&f.repo_git, &workspace, "issue-4", 1 << 20).unwrap_err();

    assert!(format!("{err:#}").contains("no branch issue-4"), "{err:#}");
}

#[test]
fn a_damaged_bundle_fails_verification() {
    let f = fixture();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    let workspace = workspace_with_bundle(&f, &["refs/heads/issue-4"]);
    fs::write(workspace.join(".sbxm-task/branch.bundle"), b"not a bundle").unwrap();

    let err = repo::fetch_bundle(&f.repo_git, &workspace, "issue-4", 1 << 20).unwrap_err();

    assert!(
        format!("{err:#}").contains("failed verification"),
        "{err:#}"
    );
}

#[test]
fn a_bundle_with_the_task_branch_is_fetched_onto_the_agent_ref() {
    let f = fixture();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    // The agent's commit lives in a clone; its bundle goes to the workspace.
    let clone = f._tmp.path().join("agent");
    git(
        f._tmp.path(),
        &[
            "clone",
            "-q",
            "-b",
            "issue-4",
            f.repo_git.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["commit", "-q", "--allow-empty", "-m", "work"]);
    let tip = git(&clone, &["rev-parse", "HEAD"]);
    let workspace = f._tmp.path().join("task").join("workspace");
    fs::create_dir_all(workspace.join(".sbxm-task")).unwrap();
    git(
        &clone,
        &[
            "bundle",
            "create",
            workspace.join(".sbxm-task/branch.bundle").to_str().unwrap(),
            "^origin/main",
            "refs/heads/issue-4",
        ],
    );

    repo::fetch_bundle(&f.repo_git, &workspace, "issue-4", 1 << 20).unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/sbxm/agent/issue-4"]),
        tip
    );
}

fn agent_commit(f: &Fixture, message: &str) -> String {
    let base = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let tree = git(&f.repo_git, &["rev-parse", &format!("{base}^{{tree}}")]);
    let agent = git(
        &f.repo_git,
        &["commit-tree", &tree, "-p", &base, "-m", message],
    );
    git(
        &f.repo_git,
        &["update-ref", "refs/sbxm/agent/issue-4", &agent],
    );
    git(&f.repo_git, &["update-ref", "refs/heads/issue-4", &base]);
    agent
}

#[test]
fn a_commit_with_nothing_to_defang_is_not_written_again() {
    use sbxm::task::rewrite;
    let f = fixture();
    let agent = agent_commit(&f, "a plain message");
    let before = git(&f.repo_git, &["count-objects"]);

    rewrite::rebuild(&f.repo_git, "main", "issue-4").unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]),
        agent
    );
    assert_eq!(git(&f.repo_git, &["count-objects"]), before);
}

#[test]
fn a_commit_with_a_mention_is_rebuilt_and_written() {
    use sbxm::task::rewrite;
    let f = fixture();
    let agent = agent_commit(&f, "thanks @someone for #12");

    rewrite::rebuild(&f.repo_git, "main", "issue-4").unwrap();

    let rebuilt = git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]);
    assert_ne!(rebuilt, agent);
    assert_eq!(git(&f.repo_git, &["cat-file", "-t", &rebuilt]), "commit");
}
