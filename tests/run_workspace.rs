//! M2a slice 7: workspace seeding (a fresh git repo with exactly one baseline
//! commit, decisions 14, 102) and diff capture for seeded and unseeded
//! contestants (decisions 14, 113). Uses the real `git` on the host.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use sbxm::run::diff;
use sbxm::seed;
use tempfile::TempDir;

/// Plain `git` for setting up and inspecting fixtures (the code under test
/// uses its own hardened invocation).
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            tmp: TempDir::new().unwrap(),
        }
    }
    fn seed(&self) -> PathBuf {
        let seed = self.tmp.path().join("seed");
        write(&seed.join("a.txt"), "alpha\n");
        write(&seed.join("sub").join("b.txt"), "bravo\n");
        seed
    }
    fn workspace(&self) -> PathBuf {
        self.tmp.path().join("ws")
    }
    fn git_dir(&self) -> PathBuf {
        self.tmp.path().join("meta").join("baseline.git")
    }
    /// Seeds `seed()` and returns the baseline commit id.
    fn seeded(&self) -> String {
        seed::seed_contestant(&self.seed(), &self.workspace(), &self.git_dir()).unwrap()
    }
}

// ---- seeding -------------------------------------------------------------

#[test]
fn seeding_copies_the_files_into_a_repo_with_one_baseline_commit() {
    let f = Fixture::new();

    let id = f.seeded();

    let ws = f.workspace();
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("sub/b.txt")).unwrap(),
        "bravo\n"
    );
    assert_eq!(git(&ws, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&ws, &["ls-files"]).lines().count(), 2);
    assert_eq!(id.len(), 40);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    // The agent's repo and the host's baseline agree on the commit.
    assert_eq!(git(&ws, &["rev-parse", "HEAD"]), id);
    // The working tree starts clean.
    assert_eq!(git(&ws, &["status", "--porcelain"]), "");
}

#[test]
fn a_seed_with_history_and_a_remote_becomes_one_commit_and_no_remote() {
    let f = Fixture::new();
    let seed = f.seed();
    git(&seed, &["init", "-q"]);
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "first"]);
    write(&seed.join("c.txt"), "charlie\n");
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "second"]);
    git(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/private/repo.git",
        ],
    );
    let old_head = git(&seed, &["rev-parse", "HEAD"]);

    f.seeded();

    let ws = f.workspace();
    assert_eq!(git(&ws, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&ws, &["remote"]), "");
    assert!(
        !std::fs::read_to_string(ws.join(".git/config"))
            .unwrap()
            .contains("example.com")
    );
    // The old history is gone, not just hidden.
    let probe = Command::new("git")
        .current_dir(&ws)
        .args(["cat-file", "-e", &old_head])
        .output()
        .unwrap();
    assert!(!probe.status.success());
    // The seed itself is untouched.
    assert_eq!(git(&seed, &["remote"]), "origin");
    assert!(ws.join("c.txt").is_file());
}

#[test]
fn ignored_files_are_copied_but_not_in_the_baseline() {
    let f = Fixture::new();
    let seed = f.seed();
    write(&seed.join(".gitignore"), "*.log\n");
    write(&seed.join("build.log"), "noise\n");

    f.seeded();

    let ws = f.workspace();
    assert!(ws.join("build.log").is_file());
    let tracked = git(&ws, &["ls-files"]);
    assert!(!tracked.contains("build.log"), "{tracked}");
    assert!(tracked.contains(".gitignore"), "{tracked}");
}

#[test]
fn an_empty_seed_still_gets_a_baseline_commit() {
    let f = Fixture::new();
    let seed = f.tmp.path().join("empty-seed");
    std::fs::create_dir_all(&seed).unwrap();

    let id = seed::seed_contestant(&seed, &f.workspace(), &f.git_dir()).unwrap();

    assert_eq!(git(&f.workspace(), &["rev-parse", "HEAD"]), id);
}

#[test]
fn a_seed_with_a_link_is_refused_and_nothing_is_created() {
    let f = Fixture::new();
    let seed = f.seed();
    let target = f.tmp.path().join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    common::dir_link(&seed.join("link"), &target);

    let err = seed::seed_contestant(&seed, &f.workspace(), &f.git_dir())
        .unwrap_err()
        .to_string();

    assert!(err.contains("symlink or junction"), "{err}");
    assert!(!f.workspace().exists() && !f.git_dir().exists());
}

#[test]
fn an_existing_workspace_is_never_overwritten() {
    let f = Fixture::new();
    write(&f.workspace().join("mine.txt"), "keep\n");

    assert!(seed::seed_contestant(&f.seed(), &f.workspace(), &f.git_dir()).is_err());

    assert_eq!(
        std::fs::read_to_string(f.workspace().join("mine.txt")).unwrap(),
        "keep\n"
    );
}

#[test]
fn a_seed_that_is_not_a_directory_is_refused() {
    let f = Fixture::new();

    let err = seed::seed_contestant(&f.tmp.path().join("nope"), &f.workspace(), &f.git_dir())
        .unwrap_err()
        .to_string();

    assert!(err.contains("nope"), "{err}");
    assert!(!f.workspace().exists());
}

// ---- diff, seeded ----------------------------------------------------------

#[test]
fn a_seeded_diff_shows_modified_new_and_deleted_files() {
    let f = Fixture::new();
    let id = f.seeded();
    let ws = f.workspace();
    write(&ws.join("a.txt"), "alpha changed\n");
    write(&ws.join("new.txt"), "brand new\n");
    std::fs::remove_file(ws.join("sub/b.txt")).unwrap();

    let patch = diff::seeded(&f.git_dir(), &ws, &id).unwrap().patch;

    assert!(patch.contains("diff --git a/a.txt b/a.txt"), "{patch}");
    assert!(patch.contains("+alpha changed"), "{patch}");
    assert!(
        patch.contains("diff --git a/new.txt b/new.txt") && patch.contains("new file mode"),
        "{patch}"
    );
    assert!(
        patch.contains("diff --git a/sub/b.txt b/sub/b.txt") && patch.contains("deleted file mode"),
        "{patch}"
    );
}

#[test]
fn a_seeded_diff_still_shows_everything_after_the_agent_commits() {
    let f = Fixture::new();
    let id = f.seeded();
    let ws = f.workspace();
    write(&ws.join("new.txt"), "brand new\n");
    write(&ws.join("a.txt"), "alpha changed\n");
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "agent work"]);
    write(&ws.join("later.txt"), "uncommitted\n");

    let patch = diff::seeded(&f.git_dir(), &ws, &id).unwrap().patch;

    for expected in ["new.txt", "a.txt", "later.txt"] {
        assert!(
            patch.contains(&format!("diff --git a/{expected} b/{expected}")),
            "{expected}: {patch}"
        );
    }
}

#[test]
fn a_seeded_diff_survives_the_agent_deleting_or_rewriting_git() {
    let f = Fixture::new();
    let id = f.seeded();
    let ws = f.workspace();
    write(&ws.join("new.txt"), "x\n");
    std::fs::remove_dir_all(ws.join(".git")).unwrap();

    let patch = diff::seeded(&f.git_dir(), &ws, &id).unwrap().patch;

    assert!(patch.contains("diff --git a/new.txt b/new.txt"), "{patch}");
    assert!(!patch.contains(".git/"), "{patch}");
}

#[test]
fn an_agent_planted_git_config_never_runs_on_the_host() {
    let f = Fixture::new();
    let id = f.seeded();
    let ws = f.workspace();
    write(&ws.join("new.txt"), "x\n");
    let marker = f.tmp.path().join("PWNED");
    let hook = format!(
        "echo pwned > \"{}\"",
        marker.to_string_lossy().replace(char::from(92), "/")
    );
    // Every code-running hook a hostile `.git/config` can carry.
    let config = format!(
        "[core]\n\tfsmonitor = {hook}\n\thooksPath = .git/evil\n\teditor = {hook}\n\tpager = {hook}\n\
         [diff]\n\texternal = {hook}\n[diff \"x\"]\n\ttextconv = {hook}\n\
         [filter \"x\"]\n\tclean = {hook}\n\tsmudge = {hook}\n"
    );
    std::fs::write(ws.join(".git/config"), config).unwrap();
    write(&ws.join(".gitattributes"), "* filter=x diff=x\n");
    write(&ws.join(".git/evil/pre-commit"), "#!/bin/sh\ntouch PWNED\n");

    let patch = diff::seeded(&f.git_dir(), &ws, &id).unwrap().patch;

    assert!(
        !marker.exists(),
        "a planted config ran a command on the host"
    );
    assert!(patch.contains("new.txt"), "{patch}");
}

#[test]
fn ignored_files_are_left_out_of_a_seeded_diff_and_other_new_files_are_in() {
    let f = Fixture::new();
    let seed = f.seed();
    write(&seed.join(".gitignore"), "*.log\n");
    let id = f.seeded();
    let ws = f.workspace();
    write(&ws.join("debug.log"), "noise\n");
    write(&ws.join("real.txt"), "signal\n");

    let patch = diff::seeded(&f.git_dir(), &ws, &id).unwrap().patch;

    assert!(
        patch.contains("real.txt") && !patch.contains("debug.log"),
        "{patch}"
    );
}

#[test]
fn an_unchanged_seeded_workspace_has_an_empty_diff() {
    let f = Fixture::new();
    let id = f.seeded();

    assert_eq!(
        diff::seeded(&f.git_dir(), &f.workspace(), &id)
            .unwrap()
            .patch,
        ""
    );
}

#[test]
fn a_binary_file_is_reported_not_dumped() {
    let f = Fixture::new();
    let id = f.seeded();
    std::fs::write(
        f.workspace().join("blob.bin"),
        [0u8, 159, 146, 150, 0, 1, 2],
    )
    .unwrap();

    let patch = diff::seeded(&f.git_dir(), &f.workspace(), &id)
        .unwrap()
        .patch;

    assert!(patch.contains("Binary files"), "{patch}");
}

// ---- diff, unseeded ----------------------------------------------------------

fn unseeded_workspace(f: &Fixture) -> PathBuf {
    let ws = f.workspace();
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

#[test]
fn an_unseeded_diff_lists_every_new_file_with_clean_paths() {
    let f = Fixture::new();
    let ws = unseeded_workspace(&f);
    write(&ws.join("a.txt"), "alpha\n");
    write(&ws.join("sub/b.txt"), "bravo\n");

    let patch = diff::unseeded(&ws).unwrap().patch;

    assert!(patch.contains("diff --git a/a.txt b/a.txt"), "{patch}");
    assert!(
        patch.contains("diff --git a/sub/b.txt b/sub/b.txt"),
        "{patch}"
    );
    assert!(
        patch.contains("+alpha") && patch.contains("+bravo"),
        "{patch}"
    );
    // No temp-dir or snapshot names leak into the paths.
    assert!(
        !patch.contains("workspace/") && !patch.contains("snap"),
        "{patch}"
    );
}

#[test]
fn an_empty_unseeded_workspace_has_an_empty_diff_and_no_error() {
    let f = Fixture::new();
    let ws = unseeded_workspace(&f);

    assert_eq!(diff::unseeded(&ws).unwrap().patch, "");
}

#[test]
fn a_git_repo_the_agent_made_is_left_out_of_an_unseeded_diff() {
    let f = Fixture::new();
    let ws = unseeded_workspace(&f);
    write(&ws.join("app.txt"), "the work\n");
    git(&ws, &["init", "-q"]);
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "agent commit"]);
    write(&ws.join("vendor/inner/x.txt"), "nested\n");
    git(&ws.join("vendor/inner"), &["init", "-q"]);

    let patch = diff::unseeded(&ws).unwrap().patch;

    assert!(patch.contains("diff --git a/app.txt b/app.txt"), "{patch}");
    assert!(patch.contains("vendor/inner/x.txt"), "{patch}");
    assert!(
        !patch.contains(".git/") && !patch.contains("/.git"),
        "{patch}"
    );
}

#[test]
fn links_in_an_unseeded_workspace_are_skipped_and_never_followed() {
    let f = Fixture::new();
    let ws = unseeded_workspace(&f);
    write(&ws.join("real.txt"), "real\n");
    let outside = f.tmp.path().join("outside");
    write(&outside.join("secret.txt"), "TOP-SECRET-HOST-DATA\n");
    common::dir_link(&ws.join("linked"), &outside);

    let result = diff::unseeded(&ws).unwrap();

    assert!(result.patch.contains("real.txt"));
    assert!(
        !result.patch.contains("TOP-SECRET-HOST-DATA"),
        "{}",
        result.patch
    );
    assert_eq!(result.skipped, [PathBuf::from("linked")]);
}

#[test]
fn a_missing_workspace_is_an_error() {
    let f = Fixture::new();

    let err = diff::unseeded(&f.tmp.path().join("gone"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("gone"), "{err}");
}
