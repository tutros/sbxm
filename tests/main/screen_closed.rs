//! Issue 667, answer 2: screen output is best-effort in every sbxm command. A failed write to
//! stdout or stderr (a broken pipe, a closed console) is ignored and never fails the command,
//! while writes to records (`task.json`, `outcome.json`, `run.log`) stay strict.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use sbxm::screen::Screen;
use sbxm::task::outcome::{self, Kind, Outcome};
use sbxm::task::record::{self, NewTask, Process, Record};
use sbxm::task::runlog::{LineSink, Tee};

use crate::common;

/// A screen whose reader is gone: every write and flush fails as a closed pipe's does.
struct Closed;

impl Write for Closed {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
}

#[derive(Default)]
struct Lines(Vec<String>);

impl LineSink for Lines {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_owned());
    }
}

#[test]
fn screen_a_write_to_a_closed_screen_is_dropped_without_an_error() {
    let mut screen = Screen::new(Closed);

    writeln!(screen, "progress").unwrap();
    screen.write_all(b"more").unwrap();
    screen.flush().unwrap();
}

#[test]
fn screen_a_task_commands_line_still_reaches_its_run_log_when_the_screen_is_closed() {
    let log = Arc::new(Mutex::new(Lines::default()));
    let mut tee = Tee::new(Screen::new(Closed), Arc::clone(&log));

    writeln!(tee, "worker: started").unwrap();
    drop(tee);

    assert_eq!(log.lock().unwrap().0, ["worker: started"]);
}

/// `sbxm` with `args` in `dir`, its stdout and stderr a pipe whose reader closed before it
/// started (`closed`), or kept as usual.
fn sbxm(
    config_dir: &Path,
    dir: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
    closed: bool,
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sbxm"));
    cmd.args(args)
        .current_dir(dir)
        .env("SBXM_CONFIG_DIR", config_dir)
        .envs(env.iter().map(|(k, v)| (k, v)));
    if closed {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        cmd.stdout(writer.try_clone().unwrap()).stderr(writer);
        let status = cmd.status().unwrap();
        return Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
    }
    cmd.output().unwrap()
}

#[test]
fn screen_config_init_with_its_screen_closed_completes_and_writes_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("sbxm");

    let out = sbxm(&config_dir, tmp.path(), &["config", "init"], &[], true);

    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(config_dir.join("config.toml").is_file());
    assert!(
        config_dir
            .join("profiles")
            .join("default")
            .join("profile.toml")
            .is_file()
    );
}

#[test]
fn screen_task_status_with_its_screen_closed_completes() {
    let env = common::Env::new();

    let out = sbxm(
        &env.config_dir(),
        env.tmp.path(),
        &["task", "status"],
        &[],
        true,
    );

    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn screen_a_task_commands_outcome_with_its_screen_closed_matches_a_normal_runs() {
    let env = common::Env::new();
    let normal = env.tmp.path().join("normal.json");
    let closed = env.tmp.path().join("closed.json");

    let shown = sbxm(
        &env.config_dir(),
        env.tmp.path(),
        &["task", "resume", "--issue", "5"],
        &[(outcome::LOOP_FILE_ENV, &normal)],
        false,
    );
    let hidden = sbxm(
        &env.config_dir(),
        env.tmp.path(),
        &["task", "resume", "--issue", "5"],
        &[(outcome::LOOP_FILE_ENV, &closed)],
        true,
    );

    assert_eq!(shown.status.code(), hidden.status.code(), "{shown:?}");
    let (outcome::Read::Outcome(shown), outcome::Read::Outcome(hidden)) =
        (outcome::read_file(&normal), outcome::read_file(&closed))
    else {
        panic!("both runs write their outcome");
    };
    assert_eq!(shown.end, Kind::Refused, "{shown:?}");
    assert_eq!(hidden, shown);
}

/// Records stay strict: a `task.json` or `outcome.json` that can't be written is an error.
#[test]
fn screen_a_record_that_cannot_be_written_is_still_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    // A file where the task's folder would be.
    let dir = tmp.path().join("issue-3");
    std::fs::write(&dir, "").unwrap();
    let rec = Record::new(
        &NewTask {
            kind: record::Kind::Issue,
            number: 3,
            repo: "o/r",
            title: "Fix 3",
            base: "main",
            branch: "sbxm/issue-3",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 0),
    );

    assert!(record::write(&dir, &rec).is_err());
    assert!(outcome::write(&dir, &Outcome::new(Kind::Ready, None, "")).is_err());
}
