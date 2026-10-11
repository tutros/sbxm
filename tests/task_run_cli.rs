//! Issue 194, PR 208 review S-1: the binary's own wiring of `sbxm task run (--issue N | --spec
//! FILE)`: clap's target group, and `--spec` alone reaching the command.

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

#[test]
fn a_run_with_both_targets_is_refused_by_clap() {
    let env = Env::new();
    let (ok, stderr) = task_run(&env, &repo_root(&env), &["--issue", "1", "--spec", "x.md"]);

    assert!(!ok);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
    assert!(stderr.contains("Usage: sbxm task run"), "{stderr}");
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
