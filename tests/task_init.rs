//! M2b slice 1: `sbxm task init [path]` writes a starter `sbxm-task.toml`
//! that is valid as written for a Rust repo, and refuses to overwrite (spec §4).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use sbxm::task::config::{FILE_NAME, TaskConfig};
use tempfile::TempDir;

fn task_init(dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir)
        // The command must not need (or touch) the real config.
        .env("SBXM_CONFIG_DIR", dir.join("no-such-config"))
        .args(["task", "init"])
        .args(args)
        .assert()
}

#[test]
fn writes_the_file_in_the_working_directory_by_default() {
    let tmp = TempDir::new().unwrap();

    let output = task_init(tmp.path(), &[]).success().get_output().clone();

    assert!(tmp.path().join(FILE_NAME).is_file());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(FILE_NAME), "{stdout}");
    assert!(
        stdout.contains("task gates --dry-run"),
        "next step is named: {stdout}"
    );
}

#[test]
fn writes_into_the_given_folder_creating_it() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("a").join("repo");

    task_init(tmp.path(), &[repo.to_str().unwrap()]).success();

    assert!(repo.join(FILE_NAME).is_file());
}

#[test]
fn the_starter_is_valid_as_written_in_a_rust_repo() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("Cargo.toml"), "[package]\n").unwrap();
    task_init(tmp.path(), &[]).success();

    let config = TaskConfig::load(tmp.path()).unwrap();

    assert_eq!(config.sandbox.profile, "sbxm-dev");
    assert_eq!(config.gates.sandbox.len(), 3);
    assert!(config.warnings.is_empty());
}

#[test]
fn the_starter_warns_that_host_gates_run_agent_code_on_this_machine() {
    let tmp = TempDir::new().unwrap();
    task_init(tmp.path(), &[]).success();

    let text = fs::read_to_string(tmp.path().join(FILE_NAME)).unwrap();

    // The warning sits right above `host = []` (spec §7), where someone about to fill it in reads it.
    let host_line = text
        .lines()
        .position(|l| l.contains("# host = []"))
        .unwrap();
    let above: Vec<&str> = text.lines().take(host_line).collect();
    let nearby = above[above.len().saturating_sub(4)..].join("\n");
    assert!(
        nearby.contains("agent-written code on this machine"),
        "{nearby}"
    );
}

#[test]
fn refuses_to_overwrite_and_leaves_the_file_alone() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join(FILE_NAME);
    fs::write(&file, "mine").unwrap();

    let output = task_init(tmp.path(), &[]).failure().get_output().clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("not overwriting it"), "{stderr}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "mine");
}
