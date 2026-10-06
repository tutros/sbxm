//! Issue 129, M-2: a `sbxm task` command that fails, or is refused, before writing any ordinary
//! progress still appends its invocation header and the exact error text to an existing task's
//! `run.log`, with the screen output and exit status unchanged.

mod common;

use std::fs;
use std::process::Command as StdCommand;

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

/// Issue 129, M-2: fallible setup evaluated after the writers are created (here,
/// `std::env::current_dir()` for `--repo-root`) must still reach `run.log`, not bypass the tee by
/// returning straight out of `main` via `?`. The current directory is removed after the child
/// process has already started (so its `chdir` into it succeeded), giving the child's own later
/// `std::env::current_dir()` call nothing to resolve.
#[test]
fn a_command_whose_current_dir_vanishes_after_start_still_logs_its_error() {
    let env = Env::new();
    save_task(&env, 42);
    let repo_root = env.tmp.path().join("vanishing");
    fs::create_dir_all(&repo_root).unwrap();

    let child = StdCommand::new(assert_cmd::cargo::cargo_bin("sbxm"))
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .current_dir(&repo_root)
        .args(["task", "gates", "--issue", "42"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    fs::remove_dir(&repo_root).unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.starts_with("Error: "), "{stderr}");

    let log = run_log(&env, "issue-42");
    assert!(log.starts_with("# "), "{log}");
    assert!(log.contains("Error: "), "{log}");
}

/// Issue 129, M-3: a targeted command whose task-log *construction* fails (not just a later
/// append) must not look like the explicit no-target case, which stays silent on purpose. It
/// warns once on the real stderr, naming the cause, instead of quietly falling back to the inert
/// logger.
#[test]
fn a_task_log_that_cannot_be_constructed_warns_once_on_real_stderr() {
    let env = Env::new();
    // Breaks `task_log::open`'s own `GlobalConfig::load` deterministically, while the command's
    // own identical load fails the same way, so the ordinary error is unaffected.
    fs::write(env.config_dir().join("config.toml"), "bogus_key = true\n").unwrap();

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--issue", "41"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("warning:"))
        .collect();
    assert_eq!(warnings.len(), 1, "{stderr}");
    assert!(warnings[0].contains("cannot open the task log"), "{stderr}");
    assert!(stderr.contains("Error: "), "{stderr}");
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
