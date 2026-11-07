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

#[test]
fn a_branch_tip_and_a_rev_come_back_together() {
    let f = fixture();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
    let main = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);

    let (tip, commit) = repo::branch_tip_and_commit(&f.repo_git, "issue-4", "main").unwrap();

    assert_eq!(tip, main);
    assert_eq!(commit, main);
}

#[test]
fn a_missing_branch_or_rev_is_named_and_an_option_like_rev_is_only_a_name() {
    let f = fixture();
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    let no_branch = repo::branch_tip_and_commit(&f.repo_git, "nope", "main").unwrap_err();
    let no_rev = repo::branch_tip_and_commit(&f.repo_git, "issue-4", "gone").unwrap_err();
    let option = repo::branch_tip_and_commit(&f.repo_git, "issue-4", "--all").unwrap_err();

    assert!(format!("{no_branch:#}").contains("nope"), "{no_branch:#}");
    assert!(format!("{no_rev:#}").contains("gone"), "{no_rev:#}");
    assert!(format!("{option:#}").contains("--all"), "{option:#}");
}

/// Runs `work` on its own thread and fails the test when it hasn't finished in `secs`.
fn within<T: Send + 'static>(secs: u64, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .expect("timed out: the batch call is blocked on a full pipe")
}

#[test]
fn a_batch_larger_than_both_pipes_resolves_every_answer_in_order() {
    let f = fixture();
    let main = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let repo_git = f.repo_git.clone();

    let found = within(20, move || {
        let revs = vec!["refs/heads/main"; 20_000];
        repo::resolve_commits(&repo_git, &revs).unwrap()
    });

    assert_eq!(found.len(), 20_000);
    assert!(found.iter().all(|c| c.as_deref() == Some(main.as_str())));
}

#[test]
fn a_rebuild_of_a_two_thousand_commit_branch_completes() {
    use sbxm::task::rewrite;
    let f = fixture();
    let base = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let tree = git(&f.repo_git, &["rev-parse", &format!("{base}^{{tree}}")]);
    let mut script = String::new();
    for n in 0..2000 {
        script.push_str(&format!(
            "commit refs/heads/many\ncommitter t <t@t> {} +0000\ndata 2\nc\n{}\nM 040000 {tree} \n\n",
            1_700_000_000 + n,
            if n == 0 { format!("from {base}") } else { String::new() }
        ));
    }
    let path = f._tmp.path().join("import");
    fs::write(&path, script).unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "git fast-import --quiet < '{}'",
            path.to_str().unwrap()
        ))
        .current_dir(&f.repo_git)
        .env("GIT_DIR", &f.repo_git)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tip = git(&f.repo_git, &["rev-parse", "refs/heads/many"]);
    git(
        &f.repo_git,
        &["update-ref", "refs/sbxm/agent/issue-4", &tip],
    );
    git(&f.repo_git, &["update-ref", "refs/heads/issue-4", &base]);
    let repo_git = f.repo_git.clone();

    within(60, move || {
        rewrite::rebuild(&repo_git, "main", "issue-4").unwrap()
    });

    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]), tip);
}

/// Decision 198: the memory a rebuild holds is bounded by a batch, not by the history. Twenty-five commits
/// of about 900 KiB each (every one under the per-commit cap, the bundle of them tiny) are
/// rebuilt in a child limited to 96 MiB of address space; the parent builds the repo outside it.
#[cfg(unix)]
#[test]
fn a_rebuild_of_twenty_big_commits_stays_within_a_small_address_space() {
    use sbxm::task::rewrite;
    if let Some(dir) = std::env::var_os("SBXM_MEMORY_CHILD") {
        rewrite::rebuild(Path::new(&dir), "main", "issue-4").unwrap();
        return;
    }
    let f = fixture();
    let base = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let tree = git(&f.repo_git, &["rev-parse", &format!("{base}^{{tree}}")]);
    let message = f._tmp.path().join("message");
    fs::write(&message, "x".repeat(900_000)).unwrap();
    let mut parent = base.clone();
    for _ in 0..25 {
        parent = git(
            &f.repo_git,
            &[
                "commit-tree",
                &tree,
                "-p",
                &parent,
                "-F",
                message.to_str().unwrap(),
            ],
        );
    }
    git(
        &f.repo_git,
        &["update-ref", "refs/sbxm/agent/issue-4", &parent],
    );
    git(&f.repo_git, &["update-ref", "refs/heads/issue-4", &base]);

    let out = Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 98304; exec \"$0\" a_rebuild_of_twenty_big_commits_stays_within_a_small_address_space --exact --test-threads=1 --nocapture")
        .arg(std::env::current_exe().unwrap())
        .arg("")
        .env("SBXM_MEMORY_CHILD", &f.repo_git)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]),
        parent
    );
}

// Review round of PR #482 (M-1): the starts that were still over the stop line.

#[test]
fn creating_a_branch_starts_its_agent_ref_at_the_same_commit() {
    let f = fixture();

    let start = repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/sbxm/agent/issue-4"]),
        start
    );
}

#[test]
fn a_branch_that_exists_is_refused_and_no_agent_ref_is_made() {
    let f = fixture();
    let main = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    git(&f.repo_git, &["update-ref", "refs/heads/issue-4", &main]);

    let err = repo::create_branch(&f.repo_git, "issue-4", "main").unwrap_err();

    assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    let agent = git(
        &f.repo_git,
        &["for-each-ref", "--format=%(refname)", "refs/sbxm/"],
    );
    assert_eq!(agent, "");
}

/// `create_branch`, then one agent commit on it in a clone, bundled into the task's workspace as
/// the sandbox leaves it. Returns the workspace and the agent's commit.
fn agent_bundle(f: &Fixture, message: &str) -> (PathBuf, String) {
    agent_bundle_with(f, |clone| {
        git(clone, &["commit", "-q", "--allow-empty", "-m", message]);
    })
}

/// [`agent_bundle`] where `work` is what the agent does in its clone.
fn agent_bundle_with(f: &Fixture, work: impl FnOnce(&Path)) -> (PathBuf, String) {
    repo::create_branch(&f.repo_git, "issue-4", "main").unwrap();
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
    work(&clone);
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
    (workspace, tip)
}

#[test]
fn fetching_and_rebuilding_delivers_the_bundles_commits() {
    use sbxm::task::rewrite;
    let f = fixture();
    let (workspace, tip) = agent_bundle(&f, "a plain message");

    let ahead = rewrite::fetch_and_rebuild(
        &f.repo_git,
        &workspace,
        "refs/heads/main",
        "issue-4",
        1 << 20,
    )
    .unwrap();

    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/sbxm/agent/issue-4"]),
        tip
    );
    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]), tip);
    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/sbxm/rebuilt/issue-4"]),
        tip
    );
    assert_eq!(ahead, 1);
}

#[test]
fn a_start_that_does_not_resolve_is_refused_before_the_fetch() {
    use sbxm::task::rewrite;
    let f = fixture();
    let (workspace, _) = agent_bundle(&f, "a plain message");
    let main = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);

    let err = rewrite::fetch_and_rebuild(
        &f.repo_git,
        &workspace,
        "refs/heads/gone",
        "issue-4",
        1 << 20,
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("refs/heads/gone"), "{err:#}");
    assert_eq!(
        git(&f.repo_git, &["rev-parse", "refs/sbxm/agent/issue-4"]),
        main
    );
    assert_eq!(git(&f.repo_git, &["rev-parse", "refs/heads/issue-4"]), main);
}

/// What `fetch_and_rebuild` says the branch is ahead by, and what `git rev-list --count` says.
fn ahead_both_ways(f: &Fixture, work: impl FnOnce(&Path)) -> (u32, u32) {
    use sbxm::task::rewrite;
    let (workspace, _) = agent_bundle_with(f, work);
    let ahead = rewrite::fetch_and_rebuild(
        &f.repo_git,
        &workspace,
        "refs/heads/main",
        "issue-4",
        1 << 20,
    )
    .unwrap();
    let counted = git(&f.repo_git, &["rev-list", "--count", "main..issue-4"]);
    (ahead, counted.parse().unwrap())
}

#[test]
fn the_count_of_a_linear_branch_with_a_defanged_commit_is_exact() {
    let f = fixture();
    let (ahead, counted) = ahead_both_ways(&f, |clone| {
        git(clone, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(
            clone,
            &["commit", "-q", "--allow-empty", "-m", "thanks @someone"],
        );
        git(clone, &["commit", "-q", "--allow-empty", "-m", "three"]);
    });
    assert_eq!((ahead, counted), (3, 3));
}

#[test]
fn the_count_includes_the_commits_of_another_branch_the_agent_merged() {
    let f = fixture();
    let main = git(&f.repo_git, &["rev-parse", "refs/heads/main"]);
    let tree = git(&f.repo_git, &["rev-parse", &format!("{main}^{{tree}}")]);
    let mut other = main;
    for message in ["o1", "o2"] {
        other = git(
            &f.repo_git,
            &["commit-tree", &tree, "-p", &other, "-m", message],
        );
    }
    git(&f.repo_git, &["update-ref", "refs/heads/other", &other]);

    let (ahead, counted) = ahead_both_ways(&f, |clone| {
        git(clone, &["commit", "-q", "--allow-empty", "-m", "mine"]);
        git(
            clone,
            &["merge", "-q", "--no-ff", "-m", "merge", "origin/other"],
        );
    });
    assert_eq!((ahead, counted), (4, 4));
}

#[test]
fn the_count_includes_a_root_commit_the_agent_merged_in() {
    let f = fixture();
    let (ahead, counted) = ahead_both_ways(&f, |clone| {
        git(clone, &["checkout", "-q", "--orphan", "loose"]);
        git(clone, &["commit", "-q", "--allow-empty", "-m", "root"]);
        git(clone, &["checkout", "-q", "issue-4"]);
        git(
            clone,
            &[
                "merge",
                "-q",
                "--allow-unrelated-histories",
                "-m",
                "merge",
                "loose",
            ],
        );
    });
    assert_eq!((ahead, counted), (2, 2));
}
