//! Issue 194, PR 208 review S-1: the binary's own wiring of `sbxm task run (--issue N | --spec
//! FILE)`: clap's target group, and `--spec` alone reaching the command. Issue 201, decision 193:
//! `--issue` and `--spec` are now both repeatable and may be combined into a list, run in order.

mod common;

use std::fs;

use assert_cmd::Command;
use common::Env;

fn task_run(env: &Env, repo_root: &std::path::Path, args: &[&str]) -> (bool, String) {
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(repo_root)
        .args(["task", "run"])
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn task_run_full(env: &Env, repo_root: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(repo_root)
        .args(["task", "run"])
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn repo_root(env: &Env) -> std::path::PathBuf {
    let root = env.tmp.path().join("checkout");
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn a_run_with_no_target_is_refused_by_clap() {
    let env = Env::new();
    let (ok, stderr) = task_run(&env, &repo_root(&env), &[]);

    assert!(!ok);
    assert!(
        stderr.contains("--issue <N>") && stderr.contains("--spec <FILE>"),
        "{stderr}"
    );
    assert!(stderr.contains("Usage: sbxm task run"), "{stderr}");
}

/// Issue 201, decision 193: `--issue` and `--spec` are no longer mutually exclusive, and each is
/// repeatable, so clap accepts both and `main` runs them as a two-task list, in the order
/// written. The checkout has no `sbxm-task.toml`, so each task fails there (no backend call), and
/// the list goes on to the second instead of stopping after the first (decision 193 point 1).
#[test]
fn a_run_with_both_kinds_of_target_runs_them_as_a_list_in_order() {
    let env = Env::new();
    let (ok, stdout, stderr) =
        task_run_full(&env, &repo_root(&env), &["--issue", "1", "--spec", "x.md"]);

    assert!(!ok);
    assert!(!stderr.contains("Usage:"), "{stderr}");
    assert!(!stderr.contains("cannot be used with"), "{stderr}");
    let first = stdout.find("task 1 of 2: --issue 1").expect(&stdout);
    let second = stdout.find("task 2 of 2: --spec").expect(&stdout);
    assert!(first < second, "{stdout}");
    assert!(stdout.contains("sbxm-task.toml"), "{stdout}");
    assert!(
        stderr.contains("2 of 2 task(s) did not reach ready"),
        "{stderr}"
    );
}

#[test]
fn a_run_with_the_same_target_flag_repeated_is_a_list_too() {
    let env = Env::new();
    let (ok, stdout, _stderr) =
        task_run_full(&env, &repo_root(&env), &["--issue", "1", "--issue", "2"]);

    assert!(!ok);
    assert!(stdout.contains("task 1 of 2: --issue 1"), "{stdout}");
    assert!(stdout.contains("task 2 of 2: --issue 2"), "{stdout}");
}

/// PR 208 review round 2, S-1: the help names the fix-round budget, not "one fix round".
#[test]
fn the_help_names_the_fix_round_budget() {
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "run", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(output.status.success());
    assert!(!help.contains("at most one fix round"), "{help}");
    assert!(help.contains("fix_rounds"), "{help}");
}

/// `--spec` alone satisfies the target group and gets past `main`'s dispatch to the command's own
/// first check. The checkout has no `sbxm-task.toml`, so the command stops there, before any
/// backend call: the test needs no `sbx`.
#[test]
fn a_run_with_only_a_spec_gets_past_the_argument_parser() {
    let env = Env::new();

    let (ok, stderr) = task_run(&env, &repo_root(&env), &["--spec", "spec.md"]);

    assert!(!ok);
    assert!(stderr.starts_with("Error: "), "{stderr}");
    assert!(stderr.contains("sbxm-task.toml"), "{stderr}");
    assert!(!stderr.contains("Usage:"), "{stderr}");
}
