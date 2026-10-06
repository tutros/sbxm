//! Issue 129, M-2: a `sbxm task` command that fails, or is refused, before writing any ordinary
//! progress still appends its invocation header and the exact error text to an existing task's
//! `run.log`, with the screen output and exit status unchanged.

mod common;

use std::fs;

use assert_cmd::Command;
use common::Env;
use sbxm::task::record::{self, Kind, NewTask, Process};

const T0: u64 = 1_790_000_000;

fn save_task(env: &Env, number: u32) {
    let record = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
        },
        T0,
        Process::new(777_777, T0),
    );
    record::write(&record::task_dir(&env.base_dir(), &record.id), &record).unwrap();
}

fn run_log(env: &Env, id: &str) -> String {
    fs::read_to_string(record::task_dir(&env.base_dir(), id).join("run.log")).unwrap()
}

#[test]
fn a_failing_gates_command_appends_its_header_and_error_to_the_existing_log() {
    let env = Env::new();
    save_task(&env, 41);
    let repo_root = env.tmp.path().join("no-config-here");
    fs::create_dir_all(&repo_root).unwrap();

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(&repo_root)
        .args(["task", "gates", "--issue", "41"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.starts_with("Error: "), "{stderr}");
    assert!(
        stderr.contains("sbxm-task.toml") && stderr.contains("sbxm task init"),
        "{stderr}"
    );

    let log = run_log(&env, "issue-41");
    assert!(log.starts_with("# "), "{log}");
    assert!(
        log.contains("sbxm-task.toml") && log.contains("sbxm task init"),
        "{log}"
    );
}

/// Issue 129, M-1: a `run.log` an earlier invocation already started still gets a header for
/// this invocation too, not just the error lines trailing after the previous one's header.
#[test]
fn a_second_invocation_that_fails_still_gets_its_own_header() {
    let env = Env::new();
    save_task(&env, 41);

    let status = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--issue", "41"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(
        run_log(&env, "issue-41")
            .lines()
            .filter(|l| l.starts_with("# "))
            .count(),
        1
    );

    let repo_root = env.tmp.path().join("no-config-here");
    fs::create_dir_all(&repo_root).unwrap();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(&repo_root)
        .args(["task", "gates", "--issue", "41"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    let log = run_log(&env, "issue-41");
    assert_eq!(
        log.lines().filter(|l| l.starts_with("# ")).count(),
        2,
        "{log}"
    );
}

#[test]
fn a_cancelled_rm_without_a_terminal_appends_its_header_and_refusal_to_the_existing_log() {
    let env = Env::new();
    save_task(&env, 41);

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "rm", "--issue", "41"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.starts_with("Error: "), "{stderr}");
    assert!(stderr.contains("pass --yes"), "{stderr}");

    let log = run_log(&env, "issue-41");
    assert!(log.starts_with("# "), "{log}");
    assert!(log.contains("pass --yes"), "{log}");
    assert!(
        record::task_dir(&env.base_dir(), "issue-41").is_dir(),
        "declining must not delete the task"
    );
}
