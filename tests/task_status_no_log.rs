//! Issue 576: the read-only `sbxm task` commands (`task status`, `task gates --dry-run`) write
//! nothing to any task's `run.log`, like `task logs` (decision 189 (c)), so `sbxm status`'s LOG
//! column keeps showing the last line a task's own phase wrote.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::Env;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage};

const T0: u64 = 1_790_000_000;

/// A phase's own lines, as `task_log`'s writer leaves them.
const PHASE_LOG: &str = "# 2026-11-11T07:00:00Z sbxm task start --issue 1 | exe sbxm | sha256 x | commit y | pid 1\n\
                         2026-11-11T07:00:01Z [working] the worker is running\n";

fn working(base: &Path, number: u32) -> PathBuf {
    let mut record = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    );
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    let dir = record::task_dir(base, &record.id);
    record::write(&dir, &record).unwrap();
    dir
}

fn sbxm(env: &Env, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(env.tmp.path())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn task_status_over_two_tasks_leaves_both_run_logs_unchanged() {
    let env = Env::new();
    let one = working(&env.base_dir(), 1);
    let two = working(&env.base_dir(), 2);
    fs::write(one.join("run.log"), PHASE_LOG).unwrap();

    let output = sbxm(&env, &["task", "status"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("issue-1") && stdout.contains("issue-2"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(one.join("run.log")).unwrap(), PHASE_LOG);
    assert!(!two.join("run.log").exists(), "task status made a run.log");
}

#[test]
fn task_status_of_one_task_leaves_its_run_log_unchanged() {
    for args in [
        &["task", "status", "--issue", "1"][..],
        &["task", "status", "--json", "--issue", "1"][..],
    ] {
        let env = Env::new();
        let dir = working(&env.base_dir(), 1);
        fs::write(dir.join("run.log"), PHASE_LOG).unwrap();

        let output = sbxm(&env, args);

        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(
            fs::read_to_string(dir.join("run.log")).unwrap(),
            PHASE_LOG,
            "{args:?}"
        );
    }
}

#[test]
fn a_failing_task_status_writes_no_run_log() {
    let env = Env::new();
    let dir = working(&env.base_dir(), 1);
    fs::write(dir.join("run.log"), PHASE_LOG).unwrap();

    let output = sbxm(&env, &["task", "status", "--issue", "9"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("Error: "), "{stderr}");
    assert_eq!(fs::read_to_string(dir.join("run.log")).unwrap(), PHASE_LOG);
    assert!(!record::task_dir(&env.base_dir(), "issue-9").exists());
}

#[test]
fn the_dashboard_log_column_shows_the_phase_line_after_a_task_status() {
    let env = Env::new();
    let one = working(&env.base_dir(), 1);
    working(&env.base_dir(), 2);
    fs::write(one.join("run.log"), PHASE_LOG).unwrap();

    assert!(sbxm(&env, &["task", "status"]).status.success());
    let output = sbxm(&env, &["status"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let row = stdout
        .lines()
        .find(|l| l.starts_with("issue-1 "))
        .unwrap_or_else(|| panic!("no issue-1 row in {stdout}"));
    assert!(row.ends_with("[working] the worker is running"), "{stdout}");
    assert!(!row.contains("issue-2"), "{stdout}");
}

#[test]
fn task_gates_dry_run_leaves_the_run_log_unchanged() {
    let env = Env::new();
    let dir = working(&env.base_dir(), 1);
    fs::write(dir.join("run.log"), PHASE_LOG).unwrap();
    fs::write(
        env.tmp.path().join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();

    let output = sbxm(&env, &["task", "gates", "--issue", "1", "--dry-run"]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("dry run"),
        "{output:?}"
    );
    assert_eq!(fs::read_to_string(dir.join("run.log")).unwrap(), PHASE_LOG);
}

#[test]
fn task_gates_dry_run_of_a_task_with_no_log_makes_none() {
    let env = Env::new();
    let dir = working(&env.base_dir(), 1);
    fs::write(
        env.tmp.path().join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();

    let output = sbxm(&env, &["task", "gates", "--issue", "1", "--dry-run"]);

    assert!(output.status.success(), "{output:?}");
    assert!(!dir.join("run.log").exists());
}

#[test]
fn task_states_makes_no_run_log() {
    let env = Env::new();
    let dir = working(&env.base_dir(), 1);

    assert!(sbxm(&env, &["task", "states"]).status.success());

    assert!(!dir.join("run.log").exists());
}
