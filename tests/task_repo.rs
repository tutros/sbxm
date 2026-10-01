//! M2b slice 5: the host-owned repo (spec §6): bare clone, task branch, workspace clone,
//! PR head fetch, push, commits ahead. A local bare repo plays GitHub.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::task::repo::{self, Identity};

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
    /// The stand-in for GitHub: a bare repo with `main` (one commit).
    origin: PathBuf,
    /// `<tmp>/task/repo.git`, not created yet.
    repo_git: PathBuf,
    /// `<tmp>/task/workspace`, not created yet.
    workspace: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    fs::create_dir_all(&origin).unwrap();
    fs::create_dir_all(&seed).unwrap();
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(&seed, &["init", "-b", "main"]);
    fs::write(seed.join("README.md"), "hello\n").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-m", "first"]);
    git(&seed, &["push", origin.to_str().unwrap(), "main"]);
    fs::create_dir_all(root.join("task")).unwrap();
    Fixture {
        origin,
        repo_git: root.join("task").join("repo.git"),
        workspace: root.join("task").join("workspace"),
        root,
        _tmp: tmp,
    }
}

fn identity() -> Identity {
    Identity {
        name: "Dev Person".into(),
        email: "dev@example.com".into(),
    }
}

fn source(f: &Fixture) -> String {
    f.origin.to_str().unwrap().to_owned()
}

#[test]
fn a_bare_clone_has_the_branches_and_remembers_its_origin() {
    let f = fixture();

    repo::clone_bare(&source(&f), &f.repo_git).unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "--is-bare-repository"]),
        "true"
    );
    assert!(!git(&f.repo_git, &["rev-parse", "refs/heads/main"]).is_empty());
    assert_eq!(
        git(&f.repo_git, &["config", "remote.origin.url"]),
        source(&f)
    );
}

#[test]
fn cloning_a_missing_source_is_an_error_naming_it() {
    let f = fixture();
    let missing = f.root.join("nope.git");

    let message = format!(
        "{:#}",
        repo::clone_bare(missing.to_str().unwrap(), &f.repo_git).unwrap_err()
    );

    assert!(message.contains("nope.git"), "{message}");
    assert!(!f.repo_git.exists());
}

#[test]
fn a_task_branch_starts_at_the_base() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();

    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]),
        git(&f.repo_git, &["rev-parse", "refs/heads/main"])
    );
}

#[test]
fn a_task_branch_off_a_missing_base_is_an_error_naming_the_base() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();

    let message = format!(
        "{:#}",
        repo::create_branch(&f.repo_git, "issue-4", "develop").unwrap_err()
    );

    assert!(message.contains("develop"), "{message}");
}

#[test]
fn the_workspace_clone_is_on_the_task_branch_with_the_hosts_identity() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    repo::clone_workspace(&f.repo_git, &f.workspace, "issue-4", &identity()).unwrap();

    assert_eq!(git(&f.workspace, &["branch", "--show-current"]), "issue-4");
    assert!(f.workspace.join("README.md").is_file());
    assert_eq!(git(&f.workspace, &["config", "user.name"]), "Dev Person");
    assert_eq!(
        git(&f.workspace, &["config", "user.email"]),
        "dev@example.com"
    );
    // `origin/<base>` exists, which the sandbox's `git bundle ... ^origin/<base>` needs.
    assert!(!git(&f.workspace, &["rev-parse", "origin/main"]).is_empty());
}

#[test]
fn the_workspace_shares_no_objects_with_the_repo() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    repo::clone_workspace(&f.repo_git, &f.workspace, "issue-4", &identity()).unwrap();

    let alternates = f
        .workspace
        .join(".git")
        .join("objects")
        .join("info")
        .join("alternates");
    assert!(
        !alternates.exists(),
        "a shared object store would let the agent reach repo.git"
    );
}

#[test]
fn only_plain_ref_names_reach_git() {
    for good in ["main", "issue-4", "feature/x_1.2", "pr-7"] {
        assert!(repo::valid_ref_name(good), "{good}");
    }
    for bad in [
        "",
        "-x",
        "--upload-pack=evil",
        "a b",
        "a..b",
        "a//b",
        "/a",
        "a/",
        ".a",
        "a.",
        "a.lock",
        "a@{b",
        "a;b",
        "a$(b)",
        "a\nb",
    ] {
        assert!(!repo::valid_ref_name(bad), "{bad:?}");
    }
}

#[test]
fn a_branch_name_that_looks_like_an_option_is_refused_before_git_runs() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    let message = format!(
        "{:#}",
        repo::create_branch(&f.repo_git, "--delete", "main").unwrap_err()
    );
    assert!(message.contains("isn't a usable branch name"), "{message}");
    let message = format!("{:#}", repo::push(&f.repo_git, "-f").unwrap_err());
    assert!(message.contains("isn't a usable branch name"), "{message}");
}

#[test]
fn the_workspace_clone_carries_no_hooks() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    // Even a hook sitting in repo.git must not be copied into the agent's clone.
    fs::create_dir_all(f.repo_git.join("hooks")).unwrap();
    fs::write(
        f.repo_git.join("hooks").join("post-checkout"),
        "#!/bin/sh\n",
    )
    .unwrap();

    repo::clone_workspace(&f.repo_git, &f.workspace, "issue-4", &identity()).unwrap();

    let hooks = f.workspace.join(".git").join("hooks");
    let count = fs::read_dir(&hooks).map_or(0, Iterator::count);
    assert_eq!(count, 0, "hooks dir should be empty or absent");
}

#[test]
fn the_identity_is_read_from_a_git_config_file() {
    let f = fixture();
    let config = f.root.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Some One\n\temail = some@one.dev\n",
    )
    .unwrap();

    let found = Identity::read_from(Some(&config)).unwrap();

    assert_eq!(
        found,
        Identity {
            name: "Some One".into(),
            email: "some@one.dev".into()
        }
    );
}

#[test]
fn a_missing_identity_says_how_to_set_it() {
    let f = fixture();
    let config = f.root.join("empty-gitconfig");
    fs::write(&config, "").unwrap();

    let message = format!("{:#}", Identity::read_from(Some(&config)).unwrap_err());

    assert!(
        message.contains("git config --global user.name"),
        "{message}"
    );
}

#[test]
fn a_pr_head_is_fetched_into_a_local_branch() {
    let f = fixture();
    // GitHub keeps a PR's head at refs/pull/<n>/head.
    let main = git(&f.origin, &["rev-parse", "main"]);
    git(&f.origin, &["update-ref", "refs/pull/7/head", &main]);
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();

    let branch = repo::fetch_pr_head(&f.repo_git, 7).unwrap();

    assert_eq!(branch, "pr-7");
    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/pr-7"]), main);
}

#[test]
fn a_missing_pr_head_is_an_error_naming_the_pr() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    let message = format!("{:#}", repo::fetch_pr_head(&f.repo_git, 99).unwrap_err());
    assert!(message.contains("99"), "{message}");
}

/// Adds a commit to `branch` of `repo_git` the way a fetched bundle would.
fn commit_on(f: &Fixture, branch: &str, file: &str) {
    let scratch = f.root.join(format!("scratch-{file}"));
    git(
        &f.root,
        &[
            "clone",
            "-q",
            f.repo_git.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    git(&scratch, &["checkout", "-q", branch]);
    fs::write(scratch.join(file), "x\n").unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", file]);
    git(&scratch, &["push", "-q", "origin", branch]);
}

#[test]
fn commits_ahead_counts_the_branch_over_the_base() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    assert_eq!(
        repo::commits_ahead(&f.repo_git, "main", "issue-4").unwrap(),
        0
    );

    commit_on(&f, "issue-4", "a.txt");
    commit_on(&f, "issue-4", "b.txt");

    assert_eq!(
        repo::commits_ahead(&f.repo_git, "main", "issue-4").unwrap(),
        2
    );
}

#[test]
fn push_sends_only_the_task_branch_to_origin() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    repo::create_branch(&f.repo_git, "other", "main").unwrap();
    commit_on(&f, "issue-4", "a.txt");

    repo::push(&f.repo_git, "issue-4").unwrap();

    let branches = git(
        &f.origin,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    );
    assert!(branches.lines().any(|b| b == "issue-4"), "{branches}");
    assert!(!branches.lines().any(|b| b == "other"), "{branches}");
    assert_eq!(
        git(&f.origin, &["rev-parse", "issue-4"]),
        git(&f.repo_git, &["rev-parse", "issue-4"])
    );
}

#[test]
fn pushing_a_missing_branch_is_an_error_naming_it() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    let message = format!("{:#}", repo::push(&f.repo_git, "issue-9").unwrap_err());
    assert!(message.contains("issue-9"), "{message}");
}
