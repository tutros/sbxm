//! M2b slice 5: the host-owned repo (spec §6): bare clone, task branch, workspace clone,
//! PR head fetch, push, commits ahead. A local bare repo plays GitHub.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::task::repo::{self, Identity};

/// Plain `git` for building fixtures (not the code under test).
fn git(dir: &Path, args: &[&str]) -> String {
    // Windows can briefly refuse git a file an indexer or antivirus holds (issue #39); the
    // production runner retries that, so the fixture does too.
    let mut out = None;
    for attempt in 0..5 {
        let run = Command::new("git")
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
        let denied = String::from_utf8_lossy(&run.stderr).contains("Permission denied");
        out = Some(run);
        if !denied {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
    }
    let out = out.unwrap();
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
fn the_workspace_clone_never_converts_line_endings() {
    // The user's own git config may say `core.autocrlf = true` (the Windows default). A
    // checkout with CRLF files looks "modified" to the Linux git in the sandbox, so `git add -A`
    // would commit whole-file line-ending changes: the clone pins the setting off.
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    repo::clone_workspace(&f.repo_git, &f.workspace, "issue-4", &identity()).unwrap();

    assert_eq!(
        git(&f.workspace, &["config", "--local", "core.autocrlf"]),
        "false"
    );
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

/// `git <args>` with `input` on stdin; returns trimmed stdout (for planting objects git won't build normally).
fn git_stdin(dir: &Path, args: &[&str], input: &[u8]) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn a_pr_head_with_a_malformed_tree_is_refused_by_fsck() {
    let f = fixture();
    // Cloned first: a local clone copies every object, which would leave nothing to fetch.
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    // A PR author can push any object GitHub accepts. Here: a tree with a `.git` entry,
    // which a checkout must never see (it would overwrite the repo's own metadata).
    let blob = git_stdin(&f.origin, &["hash-object", "-w", "--stdin"], b"payload\n");
    let mut raw = b"100644 .git\0".to_vec();
    raw.extend(
        (0..blob.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&blob[i..i + 2], 16).unwrap()),
    );
    let tree = git_stdin(
        &f.origin,
        &["hash-object", "-t", "tree", "-w", "--literally", "--stdin"],
        &raw,
    );
    let commit = git(&f.origin, &["commit-tree", &tree, "-m", "evil"]);
    git(&f.origin, &["update-ref", "refs/pull/7/head", &commit]);

    let message = format!("{:#}", repo::fetch_pr_head(&f.repo_git, 7).unwrap_err());

    assert!(message.contains("PR #7"), "{message}");
    let branches = git(
        &f.repo_git,
        &["for-each-ref", "--format=%(refname)", "refs/heads"],
    );
    assert!(
        !branches.contains("pr-7"),
        "the malformed head must not become a branch: {branches}"
    );
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

// ---- The bundle path: the one place agent-made data enters repo.git (spec §6) ----

const CAP: u64 = 50 * 1024 * 1024;

/// A prepared task: repo.git with `issue-4` at `main`, and the agent's workspace clone.
fn prepared(f: &Fixture) {
    repo::clone_bare(&source(f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    repo::clone_workspace(&f.repo_git, &f.workspace, "issue-4", &identity()).unwrap();
}

/// What the agent does in its workspace: commits on `issue-4`, then the sandbox bundles them.
fn agent_commits(f: &Fixture, files: &[&str]) {
    for file in files {
        fs::write(f.workspace.join(file), "x\n").unwrap();
        git(&f.workspace, &["add", "-A"]);
        git(&f.workspace, &["commit", "-q", "-m", file]);
    }
}

fn make_bundle(f: &Fixture, refs: &[&str]) -> PathBuf {
    let dir = f.workspace.join(".sbxm-task");
    fs::create_dir_all(&dir).unwrap();
    let bundle = dir.join("branch.bundle");
    let mut args = vec!["bundle", "create", bundle.to_str().unwrap()];
    args.extend_from_slice(refs);
    args.push("^origin/main");
    git(&f.workspace, &args);
    bundle
}

fn tip(repo_git: &Path, branch: &str) -> String {
    git(repo_git, &["rev-parse", &format!("refs/heads/{branch}")])
}

/// Collects the bundle the sandbox left in the workspace at `.sbxm-task/branch.bundle`.
fn collect(f: &Fixture, cap: u64) -> anyhow::Result<()> {
    repo::fetch_bundle(&f.repo_git, &f.workspace, "issue-4", cap)
}

/// The path the sandbox writes the bundle to, with its folder made.
fn bundle_path(f: &Fixture) -> PathBuf {
    let dir = f.workspace.join(".sbxm-task");
    fs::create_dir_all(&dir).unwrap();
    dir.join("branch.bundle")
}

#[test]
fn a_good_bundle_advances_the_task_branch() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt", "b.txt"]);
    make_bundle(&f, &["issue-4"]);

    collect(&f, CAP).unwrap();

    assert_eq!(
        tip(&f.repo_git, "issue-4"),
        git(&f.workspace, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        repo::commits_ahead(&f.repo_git, "main", "issue-4").unwrap(),
        2
    );
}

#[test]
fn a_second_bundle_on_top_of_the_first_is_accepted() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["issue-4"]);
    collect(&f, CAP).unwrap();

    agent_commits(&f, &["fix.txt"]);
    make_bundle(&f, &["issue-4"]);
    collect(&f, CAP).unwrap();

    assert_eq!(
        repo::commits_ahead(&f.repo_git, "main", "issue-4").unwrap(),
        2
    );
}

#[test]
fn garbage_that_is_not_a_bundle_is_refused() {
    let f = fixture();
    prepared(&f);
    let before = tip(&f.repo_git, "issue-4");
    fs::write(bundle_path(&f), "this is not a bundle").unwrap();

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("bundle"), "{message}");
    assert_eq!(tip(&f.repo_git, "issue-4"), before);
}

#[test]
fn a_bundle_built_on_commits_repo_git_lacks_is_refused() {
    let f = fixture();
    prepared(&f);
    // A history that never came from repo.git: the bundle needs a base it can't find.
    let alien = f.root.join("alien");
    fs::create_dir_all(&alien).unwrap();
    git(&alien, &["init", "-q", "-b", "main"]);
    fs::write(alien.join("x"), "1").unwrap();
    git(&alien, &["add", "-A"]);
    git(&alien, &["commit", "-q", "-m", "one"]);
    fs::write(alien.join("y"), "2").unwrap();
    git(&alien, &["add", "-A"]);
    git(&alien, &["commit", "-q", "-m", "two"]);
    let bundle = bundle_path(&f);
    git(
        &alien,
        &[
            "bundle",
            "create",
            bundle.to_str().unwrap(),
            "main",
            "^main~1",
        ],
    );

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("bundle"), "{message}");
}

#[test]
fn a_bundle_without_the_task_branch_is_refused_naming_it() {
    let f = fixture();
    prepared(&f);
    git(&f.workspace, &["checkout", "-q", "-b", "other"]);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["other"]);

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("issue-4"), "{message}");
}

#[test]
fn other_branches_in_a_bundle_are_not_imported() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    git(&f.workspace, &["branch", "evil"]);
    git(&f.workspace, &["tag", "v-evil"]);
    make_bundle(&f, &["issue-4", "evil", "v-evil"]);

    collect(&f, CAP).unwrap();

    let refs = git(&f.repo_git, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.contains("evil"), "{refs}");
    assert!(refs.contains("refs/heads/issue-4"), "{refs}");
}

#[test]
fn a_bundle_over_the_size_cap_is_refused_and_nothing_changes() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["issue-4"]);
    let before = tip(&f.repo_git, "issue-4");

    let message = format!("{:#}", collect(&f, 10).unwrap_err());

    assert!(
        message.contains("too large") && message.contains("10"),
        "{message}"
    );
    assert_eq!(tip(&f.repo_git, "issue-4"), before);
}

#[test]
fn a_bundle_path_that_is_not_a_plain_file_is_refused() {
    let f = fixture();
    prepared(&f);
    let target = f.root.join("somewhere");
    fs::create_dir_all(&target).unwrap();
    common::dir_link(&bundle_path(&f), &target);

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("not a plain file"), "{message}");
}

#[test]
fn a_bundle_folder_that_is_a_link_out_of_the_workspace_is_refused() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    // A perfectly good bundle, but reached through a link the agent made to another folder.
    make_bundle(&f, &["issue-4"]);
    let elsewhere = f.root.join("elsewhere");
    fs::rename(f.workspace.join(".sbxm-task"), &elsewhere).unwrap();
    common::dir_link(&f.workspace.join(".sbxm-task"), &elsewhere);
    let before = tip(&f.repo_git, "issue-4");

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("inside the workspace"), "{message}");
    assert_eq!(tip(&f.repo_git, "issue-4"), before);
}

#[test]
fn a_missing_bundle_says_the_sandbox_did_not_write_one() {
    let f = fixture();
    prepared(&f);
    let message = format!("{:#}", collect(&f, CAP).unwrap_err());
    assert!(
        message.contains("branch.bundle") && message.contains("sandbox"),
        "{message}"
    );
}

#[test]
fn the_cap_is_exact_a_bundle_of_exactly_that_size_passes_one_byte_over_does_not() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    let size = fs::metadata(make_bundle(&f, &["issue-4"])).unwrap().len();

    let message = format!("{:#}", collect(&f, size - 1).unwrap_err());
    assert!(message.contains("too large"), "{message}");

    collect(&f, size).unwrap();
}

#[cfg(unix)]
#[test]
fn a_bundle_that_is_a_symlink_to_a_host_file_is_refused() {
    let f = fixture();
    prepared(&f);
    let secret = f.root.join("host-secret.txt");
    fs::write(&secret, "top secret").unwrap();
    common::file_link(&bundle_path(&f), &secret);

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("not a plain file"), "{message}");
}

#[test]
fn rewritten_history_is_refused_and_the_branch_keeps_its_commits() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["issue-4"]);
    collect(&f, CAP).unwrap();
    let kept = tip(&f.repo_git, "issue-4");

    // The agent throws its commit away and writes a different one on the base.
    git(&f.workspace, &["reset", "-q", "--hard", "origin/main"]);
    agent_commits(&f, &["different.txt"]);
    make_bundle(&f, &["issue-4"]);

    let message = format!("{:#}", collect(&f, CAP).unwrap_err());

    assert!(message.contains("history"), "{message}");
    assert_eq!(tip(&f.repo_git, "issue-4"), kept);
}

/// Where a planted command would leave proof that it ran.
fn sentinel(f: &Fixture, name: &str) -> PathBuf {
    f.root.join(name)
}

#[test]
fn a_hostile_workspace_config_is_never_executed_by_collecting_the_bundle() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["issue-4"]);
    // The agent plants commands in its own .git/config and hooks (the #41 attack).
    let ran = sentinel(&f, "ran-config");
    let command = format!("echo ran > '{}'", ran.to_str().unwrap().replace('\\', "/"));
    let config = f.workspace.join(".git").join("config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "[core]\n\tfsmonitor = {command}\n\thooksPath = {hooks}\n[diff]\n\texternal = {command}\n\
         [filter \"x\"]\n\tclean = {command}\n\tsmudge = {command}\n",
        hooks = f
            .workspace
            .join(".git")
            .join("evil-hooks")
            .display()
            .to_string()
            .replace('\\', "/"),
    ));
    fs::write(&config, text).unwrap();
    let hooks = f.workspace.join(".git").join("evil-hooks");
    fs::create_dir_all(&hooks).unwrap();
    for hook in [
        "pre-commit",
        "post-checkout",
        "reference-transaction",
        "post-merge",
    ] {
        fs::write(hooks.join(hook), format!("#!/bin/sh\n{command}\n")).unwrap();
    }

    collect(&f, CAP).unwrap();

    assert!(
        !ran.exists(),
        "a command planted in the agent's .git ran on the host"
    );
}

#[test]
fn hooks_in_repo_git_do_not_run_on_the_fetch_path() {
    let f = fixture();
    prepared(&f);
    agent_commits(&f, &["a.txt"]);
    make_bundle(&f, &["issue-4"]);
    let ran = sentinel(&f, "ran-hook");
    let command = format!("echo ran > '{}'", ran.to_str().unwrap().replace('\\', "/"));
    let hooks = f.repo_git.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for hook in [
        "reference-transaction",
        "post-update",
        "post-merge",
        "pre-auto-gc",
    ] {
        fs::write(hooks.join(hook), format!("#!/bin/sh\n{command}\n")).unwrap();
    }

    collect(&f, CAP).unwrap();

    assert!(!ran.exists(), "a hook ran during the fetch");
}

#[test]
fn hostile_committed_content_runs_nothing_when_fetched() {
    let f = fixture();
    prepared(&f);
    let ran = sentinel(&f, "ran-content");
    let command = format!("echo ran > '{}'", ran.to_str().unwrap().replace('\\', "/"));
    // Committed files can name a filter and a submodule URL; neither may execute anything.
    fs::write(
        f.workspace.join(".gitattributes"),
        "* filter=evil diff=evil\n",
    )
    .unwrap();
    fs::write(
        f.workspace.join(".gitmodules"),
        format!("[submodule \"s\"]\n\tpath = s\n\turl = ext::sh -c \"{command}\"\n"),
    )
    .unwrap();
    fs::create_dir_all(f.workspace.join("hooks")).unwrap();
    fs::write(
        f.workspace.join("hooks").join("post-merge"),
        format!("#!/bin/sh\n{command}\n"),
    )
    .unwrap();
    git(&f.workspace, &["add", "-A"]);
    git(&f.workspace, &["commit", "-q", "-m", "hostile content"]);
    make_bundle(&f, &["issue-4"]);

    collect(&f, CAP).unwrap();

    assert!(
        !ran.exists(),
        "committed content made the host run a command"
    );
}

#[test]
fn a_task_branch_that_already_exists_on_origin_is_refused_in_plain_words() {
    let f = fixture();
    repo::clone_bare(&source(&f), &f.repo_git).unwrap();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    let message = format!(
        "{:#}",
        repo::create_branch(&f.repo_git, "issue-4", "main").unwrap_err()
    );

    assert!(
        message.contains("issue-4 already exists in the repo"),
        "{message}"
    );
    assert!(message.contains("delete it"), "{message}");
    assert!(!message.contains("failed"), "{message}");
}
