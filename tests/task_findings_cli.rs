//! M2b slice 11, step 5: `sbxm task file-findings` on the command line (spec §4). Only what
//! needs no GitHub: the arguments and the first refusals.

mod common;

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn sbxm(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir)
        // The command must not need (or touch) the real config.
        .env("SBXM_CONFIG_DIR", dir.join("no-such-config"))
        .args(["task", "file-findings"])
        .args(args)
        .assert()
        .failure()
        .get_output()
        .clone()
}

#[test]
fn a_review_file_is_required() {
    let dir = TempDir::new().unwrap();
    let stderr = String::from_utf8(sbxm(dir.path(), &[]).stderr).unwrap();
    assert!(stderr.contains("--file"), "{stderr}");
}

#[test]
fn a_missing_file_is_refused_in_one_line() {
    let dir = TempDir::new().unwrap();
    let stderr =
        String::from_utf8(sbxm(dir.path(), &["--file", "nope.md", "--repo", "o/r"]).stderr)
            .unwrap();
    assert!(
        stderr.contains("review file nope.md not found; give the path of a file in sdlc/reviews/"),
        "{stderr}"
    );
}

/// Issue 129, M-4: `--file` has no task to log into, so there is nothing to warn about even
/// though the command itself fails.
#[test]
fn a_no_target_command_emits_no_logging_warning() {
    let dir = TempDir::new().unwrap();
    let stderr =
        String::from_utf8(sbxm(dir.path(), &["--file", "nope.md", "--repo", "o/r"]).stderr)
            .unwrap();
    assert!(!stderr.contains("warning:"), "{stderr}");
}

#[test]
fn the_help_says_it_is_a_dry_run_unless_create() {
    let dir = TempDir::new().unwrap();
    let out = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir.path())
        .env("SBXM_CONFIG_DIR", dir.path().join("no-such-config"))
        .args(["task", "file-findings", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains("--create") && help.contains("dry run"),
        "{help}"
    );
}

#[test]
fn exactly_one_source_is_allowed() {
    let dir = TempDir::new().unwrap();
    let stderr =
        String::from_utf8(sbxm(dir.path(), &["--file", "a.md", "--issue", "3"]).stderr).unwrap();
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn a_task_that_does_not_exist_is_refused_in_one_line() {
    let env = common::Env::new();
    let out = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(env.tmp.path())
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "file-findings", "--issue", "12"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("no task issue-12; run `sbxm task status` to list the tasks"),
        "{stderr}"
    );
}
