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

/// Issue 232, boundary case 1 of 4, through the real CLI binary: a missing `run.log` keeps the
/// message it always had, and exits 1.
#[test]
fn the_cli_command_reports_a_missing_log_as_not_there_yet_and_exits_1() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let dir = record::task_dir(&env.base_dir(), &task.id);

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "logs", "--issue", "41"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains(&format!(
            "task issue-41 has no run.log yet; see {}",
            dir.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("cannot open"), "{stderr}");
}

/// Issue 232, boundary case 3 of 4: a `run.log` that is a folder is not "missing"; the error
/// names the path and the operating system's own text for it (on Windows opening the folder
/// fails, on Linux reading it does: both are reported the same way).
#[test]
fn a_log_that_is_a_folder_is_reported_with_its_path_and_the_os_error() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::create_dir(&log).unwrap();
    let os_error = fs::read(&log).unwrap_err().to_string();

    let (result, out) = run_logs(&env, Target::Issue(41), false, &task_logs::Env::real());

    let message = format!("{:#}", result.unwrap_err());
    assert_eq!(
        message,
        format!("cannot open {}: {os_error}", log.display())
    );
    assert!(out.is_empty());
}

/// Issue 232, boundary case 2 of 4: a `run.log` the user may not read. Skipped where the
/// permission can't take effect (running as root).
#[cfg(unix)]
#[test]
fn an_unreadable_log_is_reported_with_its_path_and_the_os_error() {
    use std::os::unix::fs::PermissionsExt as _;

    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::write(&log, "secret\n").unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o000)).unwrap();
    let Err(os_error) = fs::File::open(&log) else {
        eprintln!("skipped: the permission has no effect for this user");
        return;
    };

    let (result, out) = run_logs(&env, Target::Issue(41), true, &task_logs::Env::real());

    fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
    let message = format!("{:#}", result.unwrap_err());
    assert_eq!(
        message,
        format!("cannot open {}: {os_error}", log.display())
    );
    assert!(!message.contains("no run.log yet"), "{message}");
    assert!(out.is_empty());
}

/// Issue 232, boundary case 4 of 4, through the real CLI binary: an open error other than
/// not-found exits 1 and names the path, never the "no run.log yet" message.
#[test]
fn the_cli_command_reports_any_other_open_error_with_the_path_and_exits_1() {
    let env = Env::new();
    let task = save_task(&env.base_dir(), Kind::Issue, 41);
    let log = record::task_dir(&env.base_dir(), &task.id).join("run.log");
    fs::create_dir(&log).unwrap();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "logs", "--issue", "41"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains(&format!("cannot open {}: ", log.display())),
        "{stderr}"
    );
    assert!(!stderr.contains("no run.log yet"), "{stderr}");
}
