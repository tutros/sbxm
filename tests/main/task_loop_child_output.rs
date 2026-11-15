//! Issue 667, answer 1: a child `sbxm` the loop starts no longer inherits the loop's output. Its
//! stdout and stderr go to a log of its own under `<base>/.sbxm/loop-logs/children/`, so nothing
//! of the loop's (or of the trigger that started the loop) is held open by a child, and a child
//! of a killed loop writes on and ends with its true outcome.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use sbxm::backend::FakeBackend;
use sbxm::commands::task_loop::child::{Child, ChildSpawner, Exit, Spawner};
use sbxm::commands::task_loop::{PhaseEnd, Steps, TaskSteps};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::outcome::Kind;
use sbxm::task::record::{self, NewTask, Process, Record, Stage, Status};

use crate::common;
use crate::common::task_fixture::Probe;

/// Records the children it is asked to start; each exits at once.
#[derive(Default)]
struct Recorded(Mutex<Vec<Child>>);

impl Spawner for Recorded {
    fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
        self.0.lock().unwrap().push(child.clone());
        Ok(Exit {
            success: false,
            how: "exited with code 1".into(),
            errors: String::new(),
        })
    }

    fn stop(&self) {}
}

struct Fixture {
    env: common::Env,
    repo_root: PathBuf,
}

fn fixture() -> Fixture {
    let env = common::Env::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    Fixture { env, repo_root }
}

/// Issue 3's task, at `working`.
fn working(base: &Path) {
    let mut rec = Record::new(
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
    rec.stage = Stage::Working;
    rec.status = Status::Failed;
    record::write(&record::task_dir(base, "issue-3"), &rec).unwrap();
}

fn child_log(base: &Path, id: &str) -> PathBuf {
    base.join(".sbxm")
        .join("loop-logs")
        .join("children")
        .join(format!("{id}.log"))
}

fn resume_with(f: &Fixture, spawner: &dyn Spawner) -> (PhaseEnd, String) {
    let config_dir = f.env.config_dir();
    let (gh, backend, host) = (
        FakeGitHub::default(),
        FakeBackend::default(),
        FakeHostRunner::default(),
    );
    let steps = TaskSteps {
        config_dir: &config_dir,
        repo_root: f.repo_root.clone(),
        repo: Some("o/r".into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
        spawner,
    };
    let mut out = Vec::new();
    let ended = steps.resume_task(3, &mut out, &mut Vec::new());
    (ended, String::from_utf8(out).unwrap())
}

#[test]
fn task_loop_child_output_goes_to_a_log_of_the_tasks_own_under_the_loop_logs() {
    let f = fixture();
    working(&f.env.base_dir());
    let spawner = Recorded::default();

    resume_with(&f, &spawner);

    let started = spawner.0.lock().unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].log, child_log(&f.env.base_dir(), "issue-3"));
}

#[test]
fn task_loop_child_output_of_the_real_binary_reaches_its_log_and_its_error_is_still_quoted() {
    let f = fixture();
    working(&f.env.base_dir());
    // Resumed from a checkout without `sbxm-task.toml`: the child fails, saying why.
    fs::remove_file(f.repo_root.join("sbxm-task.toml")).unwrap();
    let spawner = ChildSpawner::new(PathBuf::from(env!("CARGO_BIN_EXE_sbxm")));

    let (ended, out) = resume_with(&f, &spawner);

    let PhaseEnd::Outcome(o) = &ended else {
        panic!("{ended:?}")
    };
    assert_eq!(o.end, Kind::Failed, "{o:?}");
    let log = fs::read_to_string(child_log(&f.env.base_dir(), "issue-3")).unwrap();
    assert!(log.contains("task resume --issue 3"), "a header: {log}");
    assert!(log.contains("there is no sbxm-task.toml"), "{log}");
    // The window still shows one line per event of the child.
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "[issue-3 local] task resume started",
            "[issue-3 local] task resume ended: failed"
        ],
        "{out}"
    );
}

#[cfg(unix)]
fn sh(script: &str, dir: &Path, log: &Path) -> Child {
    Child {
        args: vec!["-c".into(), script.into()],
        dir: dir.to_owned(),
        env: Vec::new(),
        log: log.to_owned(),
    }
}

/// The child's stdout and stderr are its log, never the loop's: a test on the spawner's
/// settings, read from the child's own file descriptors.
#[cfg(target_os = "linux")]
#[test]
fn task_loop_child_output_its_stdout_and_stderr_are_its_log_not_the_loops() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("logs").join("issue-3.log");
    let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));

    let exit = spawner
        .run(
            &sh(
                "readlink /proc/self/fd/1; readlink /proc/self/fd/2 >&2",
                tmp.path(),
                &log,
            ),
            &|| {},
        )
        .unwrap();

    assert!(exit.success, "{exit:?}");
    let text = fs::read_to_string(&log).unwrap();
    let canonical = fs::canonicalize(&log).unwrap();
    let fds: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(fds.len(), 2, "{text}");
    for fd in fds {
        assert_eq!(Path::new(fd), canonical, "{text}");
    }
}

/// A log kept from an earlier child is added to, and only this child's output is quoted.
#[cfg(unix)]
#[test]
fn task_loop_child_output_is_added_to_its_log_and_only_this_childs_error_is_quoted() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("issue-3.log");
    fs::write(&log, "Error: an earlier child's\n").unwrap();
    let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));

    let exit = spawner
        .run(
            &sh(
                "echo progress; echo 'Error: gone' >&2; exit 3",
                tmp.path(),
                &log,
            ),
            &|| {},
        )
        .unwrap();

    assert_eq!(exit.how, "exited with code 3");
    assert!(exit.errors.contains("Error: gone"), "{exit:?}");
    assert!(!exit.errors.contains("earlier"), "{exit:?}");
    let text = fs::read_to_string(&log).unwrap();
    assert!(text.starts_with("Error: an earlier child's\n"), "{text}");
    assert!(text.contains("progress\n"), "{text}");
    assert!(text.contains("Error: gone\n"), "{text}");
}

/// A log that can't be opened is the child's start failing, said, never output dropped silently.
#[cfg(unix)]
#[test]
fn task_loop_child_output_a_log_that_cannot_be_opened_fails_the_start() {
    let tmp = tempfile::tempdir().unwrap();
    // A file where the log's folder would be.
    fs::write(tmp.path().join("logs"), "").unwrap();
    let log = tmp.path().join("logs").join("issue-3.log");
    let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));

    let err = spawner
        .run(&sh("exit 0", tmp.path(), &log), &|| {})
        .unwrap_err();

    assert!(format!("{err:#}").contains("issue-3.log"), "{err:#}");
}
