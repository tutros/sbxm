//! Issue 574 through the binary: an error or hint that names a command names it by the running
//! executable, and is followed once by the block saying where to run it; a command that names
//! none prints no block.

use crate::common;

use assert_cmd::Command;
use common::Env;
use sbxm::hint::Shell;

fn sbxm() -> Command {
    Command::cargo_bin("sbxm").unwrap()
}

/// The program a hint names: the binary under test, quoted for this platform's shell.
fn program() -> String {
    let exe = real_path(assert_cmd::cargo::cargo_bin("sbxm"));
    Shell::HOST.program(&exe.to_string_lossy())
}

/// A working directory with a space and a single quote in its name.
fn workdir(env: &Env) -> std::path::PathBuf {
    let dir = env.tmp.path().join("my 'repo");
    std::fs::create_dir_all(&dir).unwrap();
    real_path(dir)
}

/// `path` as the process sees itself: links resolved (`current_exe` and `current_dir` do on
/// Unix), but no `\\?\` prefix on Windows, where only the separators are made the platform's
/// own (cargo names the binary `target\debug/sbxm.exe`, `current_exe` gives all backslashes).
fn real_path(path: std::path::PathBuf) -> std::path::PathBuf {
    if cfg!(windows) {
        path.components().collect()
    } else {
        std::fs::canonicalize(path).unwrap()
    }
}

#[test]
fn an_error_names_its_command_by_the_executable_and_says_where_to_run_it() {
    let env = Env::new();
    let dir = workdir(&env);
    let output = sbxm()
        .current_dir(&dir)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--issue", "5"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let shell = Shell::HOST;
    assert_eq!(
        stderr,
        format!(
            "Error: no task issue-5; run `{} task status` to list the tasks\n\
             to run the commands above as this one ran, first:\n  {}\n  {}\n",
            program(),
            shell.cd(&dir.to_string_lossy()),
            shell.set_config_dir(&env.config_dir().to_string_lossy()),
        )
    );
}

#[test]
fn a_hint_on_the_screen_is_rewritten_and_the_block_follows_once() {
    let env = Env::new();
    let dir = workdir(&env);
    let output = sbxm()
        .current_dir(&dir)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "No tasks yet; start one with `{} task start --issue <n>`.\n",
            program()
        )
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.matches("to run the commands above").count(),
        1,
        "{stderr}"
    );
    assert!(
        stderr.contains(&Shell::HOST.cd(&dir.to_string_lossy())),
        "{stderr}"
    );
}

#[test]
fn json_output_is_not_rewritten() {
    let env = Env::new();
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(&program()), "{stdout}");
}

#[test]
fn a_command_that_names_no_command_prints_no_block() {
    let env = Env::new();
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["config", "profiles-dir"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn a_task_command_writes_the_rewritten_error_and_the_block_into_its_run_log() {
    use sbxm::task::record::{self, Kind, NewTask, Process};
    let env = Env::new();
    let task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        1_790_000_000,
        Process::new(777_777, 1_790_000_000),
    );
    let dir = record::task_dir(&env.base_dir(), &task.id);
    record::write(&dir, &task).unwrap();
    let here = workdir(&env);

    sbxm()
        .current_dir(&here)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "gates", "--issue", "41"])
        .assert()
        .failure();

    let log = std::fs::read_to_string(dir.join("run.log")).unwrap();
    assert!(log.contains(&format!("`{} task init`", program())), "{log}");
    assert!(
        log.contains(&Shell::HOST.cd(&here.to_string_lossy())),
        "{log}"
    );
}
