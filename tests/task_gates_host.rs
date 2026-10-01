//! M2b slice 7, step 2: the host gate tier (spec §7, decision 160). Host gates run agent-written
//! code on this machine, so they only ever run in a clean checkout of committed objects from the
//! host-owned repo, through a `HostRunner` (faked in most tests).

mod common;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::git;
use common::task_fixture::{Fixture, fixture, source};
use sbxm::task::gates::{FakeHostRunner, HostRunner, ShellHostRunner, run_host_tier};
use sbxm::task::repo::{self, Identity};

fn cmds(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn each_command_runs_in_the_checkout_under_the_timeout_and_is_recorded_as_host() {
    let runner = FakeHostRunner::default();

    let outcomes = run_host_tier(
        &runner,
        std::path::Path::new("/c/gates"),
        "after-worker",
        &cmds(&["cargo test", "cargo build"]),
        Duration::from_secs(90),
    );

    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|o| o.result.tier == "host" && o.result.passed)
    );
    assert_eq!(outcomes[0].result.phase, "after-worker");
    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].command, "cargo test");
    assert_eq!(calls[0].cwd, PathBuf::from("/c/gates"));
    assert_eq!(calls[0].timeout, Duration::from_secs(90));
}

#[test]
fn the_first_failing_host_command_stops_the_tier() {
    let runner = FakeHostRunner::default().with_exit("build", 2, "", "boom\n");

    let outcomes = run_host_tier(
        &runner,
        std::path::Path::new("/c/gates"),
        "after-worker",
        &cmds(&["cargo test", "cargo build", "cargo doc"]),
        Duration::from_secs(90),
    );

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[1].result.exit, Some(2));
    assert!(!outcomes[1].result.passed);
    assert!(outcomes[1].output_tail.contains("boom"));
    assert_eq!(runner.calls().len(), 2, "cargo doc must not run");
}

#[test]
fn a_host_command_that_times_out_or_cannot_start_is_a_failure() {
    let runner = FakeHostRunner::default().with_timeout("slow");
    let outcomes = run_host_tier(
        &runner,
        std::path::Path::new("/c"),
        "p",
        &cmds(&["slow"]),
        Duration::from_secs(1),
    );
    assert!(outcomes[0].timed_out && !outcomes[0].result.passed);

    let runner = FakeHostRunner::default().with_start_error("nope", "no such shell");
    let outcomes = run_host_tier(
        &runner,
        std::path::Path::new("/c"),
        "p",
        &cmds(&["nope", "next"]),
        Duration::from_secs(1),
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].result.exit, None);
    assert!(
        outcomes[0].output_tail.contains("no such shell"),
        "{}",
        outcomes[0].output_tail
    );
}

// ---- The clean checkout ----

fn prepared_with_commit(f: &Fixture) -> (PathBuf, PathBuf) {
    let repo_git = f.env.tmp.path().join("task").join("repo.git");
    let workspace = f.env.tmp.path().join("task").join("ws");
    repo::clone_bare(&source(f), &repo_git).unwrap();
    repo::create_branch(&repo_git, "issue-4", "main").unwrap();
    repo::clone_workspace(
        &repo_git,
        &workspace,
        "issue-4",
        &Identity {
            name: "Dev".into(),
            email: "d@e".into(),
        },
    )
    .unwrap();
    fs::write(workspace.join("committed.txt"), "in a commit\n").unwrap();
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-q", "-m", "work"]);
    fs::create_dir_all(workspace.join(".sbxm-task")).unwrap();
    git(
        &workspace,
        &[
            "bundle",
            "create",
            ".sbxm-task/branch.bundle",
            "issue-4",
            "^origin/main",
        ],
    );
    repo::fetch_bundle(&repo_git, &workspace, "issue-4", repo::BUNDLE_CAP).unwrap();
    // Left in the agent's folder, never committed: must not reach the checkout.
    fs::write(workspace.join("untracked.txt"), "only in the workspace\n").unwrap();
    (repo_git, workspace)
}

#[test]
fn the_clean_checkout_has_only_committed_files_from_the_task_branch() {
    let f = fixture();
    let (repo_git, _workspace) = prepared_with_commit(&f);
    let dest = f.env.tmp.path().join("task").join("gates");

    repo::clean_checkout(&repo_git, "issue-4", &dest).unwrap();

    assert_eq!(
        fs::read_to_string(dest.join("committed.txt")).unwrap(),
        "in a commit\n"
    );
    assert!(dest.join("README.md").is_file());
    assert!(!dest.join("untracked.txt").exists());
    assert!(!dest.join(".sbxm-task").exists());
    assert_eq!(git(&dest, &["branch", "--show-current"]), "issue-4");
}

#[test]
fn the_clean_checkout_gets_no_hooks_and_never_converts_line_endings() {
    let f = fixture();
    let (repo_git, _) = prepared_with_commit(&f);
    fs::create_dir_all(repo_git.join("hooks")).unwrap();
    fs::write(repo_git.join("hooks").join("post-checkout"), "#!/bin/sh\n").unwrap();
    let dest = f.env.tmp.path().join("task").join("gates");

    repo::clean_checkout(&repo_git, "issue-4", &dest).unwrap();

    let hooks = dest.join(".git").join("hooks");
    assert_eq!(fs::read_dir(&hooks).map_or(0, Iterator::count), 0);
    assert_eq!(git(&dest, &["config", "--local", "core.autocrlf"]), "false");
}

#[test]
fn a_checkout_over_an_existing_folder_is_refused_and_leaves_it_alone() {
    let f = fixture();
    let (repo_git, _) = prepared_with_commit(&f);
    let dest = f.env.tmp.path().join("task").join("gates");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("mine.txt"), "keep\n").unwrap();

    let message = format!(
        "{:#}",
        repo::clean_checkout(&repo_git, "issue-4", &dest).unwrap_err()
    );

    assert!(message.contains("already exists"), "{message}");
    assert_eq!(fs::read_to_string(dest.join("mine.txt")).unwrap(), "keep\n");
}

#[test]
fn a_checkout_of_a_missing_branch_is_an_error_naming_it() {
    let f = fixture();
    let (repo_git, _) = prepared_with_commit(&f);
    let dest = f.env.tmp.path().join("task").join("gates");

    let message = format!(
        "{:#}",
        repo::clean_checkout(&repo_git, "issue-9", &dest).unwrap_err()
    );

    assert!(message.contains("issue-9"), "{message}");
    assert!(!dest.exists());
}

#[test]
fn a_branch_name_that_looks_like_an_option_is_refused_before_git_runs() {
    let f = fixture();
    let (repo_git, _) = prepared_with_commit(&f);
    let dest = f.env.tmp.path().join("task").join("gates");
    let message = format!(
        "{:#}",
        repo::clean_checkout(&repo_git, "--upload-pack=x", &dest).unwrap_err()
    );
    assert!(message.contains("isn't a usable branch name"), "{message}");
}

// ---- The real platform shell ----

#[test]
fn the_shell_runner_captures_output_and_the_exit_code() {
    let dir = tempfile::tempdir().unwrap();

    let out = ShellHostRunner
        .run(dir.path(), "echo hello-from-gate", Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout.contains("hello-from-gate"), "{out:?}");
    assert!(!out.timed_out);

    let out = ShellHostRunner
        .run(dir.path(), "exit 3", Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.exit_code, Some(3), "{out:?}");
}

#[test]
fn the_shell_runner_runs_in_the_given_folder() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("marker.txt"), "x").unwrap();
    let command = if cfg!(windows) { "dir /b" } else { "ls" };

    let out = ShellHostRunner
        .run(dir.path(), command, Duration::from_secs(30))
        .unwrap();

    assert!(out.stdout.contains("marker.txt"), "{out:?}");
}

#[test]
fn the_shell_runner_stops_a_command_that_outlives_its_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let command = if cfg!(windows) {
        "ping -n 30 127.0.0.1 > nul"
    } else {
        "sleep 30"
    };
    let started = Instant::now();

    let out = ShellHostRunner
        .run(dir.path(), command, Duration::from_secs(1))
        .unwrap();

    assert!(out.timed_out, "{out:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "took {:?}",
        started.elapsed()
    );
}
