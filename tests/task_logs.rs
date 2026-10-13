//! `sbxm task logs (--issue N | --pr N | --spec FILE) [--follow]` (decision 189, issue 88, part
//! 2 of 6 of decisions 170/171): prints a task's `run.log`. Read-only.

mod common;

use std::cell::Cell;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::rc::Rc;

use common::Env;
use sbxm::commands::task_logs::{self, Options, Target};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

const T0: u64 = 1_790_000_000;

fn save_task(base: &Path, kind: Kind, number: u32) -> Record {
    let record = Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title: "title",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    );
    record::write(&record::task_dir(base, &record.id), &record).unwrap();
    record
}

fn run_logs(
    env: &Env,
    target: Target,
    follow: bool,
    follow_env: &task_logs::Env,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_logs::run(
        &env.config_dir(),
        &Options { target, follow },
        follow_env,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn it_prints_the_existing_run_log_with_utf8_bytes_intact() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(
        &log,
        "# header\n2026-10-13T00:00:00Z [working] café \u{1F600}\n",
    )
    .unwrap();

    let (result, out) = run_logs(&env, Target::Issue(41), false, &task_logs::Env::real());

    result.unwrap();
    assert_eq!(
        out,
        "# header\n2026-10-13T00:00:00Z [working] café \u{1F600}\n"
    );
}

#[test]
fn a_pr_task_s_log_is_read_the_same_way() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Pr, 7);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(&log, "# header\nline\n").unwrap();

    let (result, out) = run_logs(&env, Target::Pr(7), false, &task_logs::Env::real());

    result.unwrap();
    assert_eq!(out, "# header\nline\n");
}

#[test]
fn a_spec_task_s_log_is_read_the_same_way() {
    let env = Env::new();
    let spec = env.tmp.path().join("my-spec.md");
    fs::write(&spec, "do the thing").unwrap();
    let (id, _) = record::spec_id(&spec).unwrap();
    let task = save_task(&env.base_dir(), Kind::Issue, 0);
    // spec tasks don't derive their id from `number`; write a record under the real spec id.
    let mut spec_record = task.clone();
    spec_record.id = id.clone();
    spec_record.kind = Kind::Spec;
    record::write(&record::task_dir(&env.base_dir(), &id), &spec_record).unwrap();
    let log = record::task_dir(&env.base_dir(), &id).join("run.log");
    fs::write(&log, "# header\nspec line\n").unwrap();

    let (result, out) = run_logs(&env, Target::Spec(spec), false, &task_logs::Env::real());

    result.unwrap();
    assert_eq!(out, "# header\nspec line\n");
}

#[test]
fn an_unknown_task_is_refused_with_the_status_hint() {
    let env = Env::new();

    let (result, _) = run_logs(&env, Target::Issue(41), false, &task_logs::Env::real());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("no task issue-41"), "{message}");
    assert!(message.contains("sbxm task status"), "{message}");
}

#[test]
fn a_task_without_a_log_names_the_folder_and_fails() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let dir = record::task_dir(&env.base_dir(), &task.id);

    let (result, out) = run_logs(&env, Target::Issue(41), false, &task_logs::Env::real());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("issue-41"), "{message}");
    assert!(message.contains(&dir.display().to_string()), "{message}");
    assert!(out.is_empty());
}

#[test]
fn without_follow_it_does_not_wait_for_new_lines() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(&log, "one\n").unwrap();
    let slept = Rc::new(Cell::new(false));
    let slept_flag = Rc::clone(&slept);
    let follow_env = task_logs::Env {
        sleep: Box::new(move |_| slept_flag.set(true)),
        iterations: Some(1),
    };

    let (result, out) = run_logs(&env, Target::Issue(41), false, &follow_env);

    result.unwrap();
    assert_eq!(out, "one\n");
    assert!(!slept.get());
}

#[test]
fn follow_prints_lines_appended_after_it_starts_until_the_test_stops_it() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(&log, "one\n").unwrap();
    let log_for_sleep = log.clone();
    let polls = Cell::new(0u32);
    let follow_env = task_logs::Env {
        sleep: Box::new(move |_| {
            let n = polls.get();
            polls.set(n + 1);
            if n == 0 {
                let mut file = fs::OpenOptions::new()
                    .append(true)
                    .open(&log_for_sleep)
                    .unwrap();
                file.write_all(b"two\n").unwrap();
            }
        }),
        iterations: Some(2),
    };

    let (result, out) = run_logs(&env, Target::Issue(41), true, &follow_env);

    result.unwrap();
    assert_eq!(out, "one\ntwo\n");
}

#[test]
fn follow_does_not_wait_for_a_missing_log_to_appear() {
    let env = Env::new();
    save_task(&env.base_dir(), Kind::Issue, 41);

    let (result, _) = run_logs(&env, Target::Issue(41), true, &task_logs::Env::real());

    assert!(result.is_err());
}

/// Decided point 3: unlike every other `sbxm task` command (`tests/task_dispatch_errors.rs`),
/// `task logs` never appends a header or anything else to the log it is printing, through the
/// real CLI binary (so `main`'s dispatch, not just the command function, is covered).
#[test]
fn the_cli_command_never_writes_to_the_run_log_it_prints() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(&log, "# header\nline\n").unwrap();
    let before = fs::read(&log).unwrap();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "logs", "--issue", "41"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, before);
    assert_eq!(fs::read(&log).unwrap(), before);
}

/// Decided point 4, through the real CLI binary: an unknown task exits 1, names the `task
/// status` hint, and creates no task folder or log of its own.
#[test]
fn the_cli_command_refuses_an_unknown_task_and_writes_nothing() {
    let env = Env::new();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "logs", "--issue", "41"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("no task issue-41") && stderr.contains("sbxm task status"));
    assert!(!record::task_dir(&env.base_dir(), "issue-41").exists());
}
