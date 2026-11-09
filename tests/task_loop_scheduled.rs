//! Issue 527 (decision 310, slice 3 of #476): the loop starts by itself at logon and keeps
//! running. `task loop --wait` checks again every `PAUSE_POLL` on an empty queue instead of
//! stopping; `task loop --stop` asks a running loop to stop as Ctrl-C does; a stop that needs the
//! user exits 3; and a task a loop was running when it died is resumed once at the next start.
//! The phases are scripted; the issues and PRs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use assert_cmd::Command;
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, PAUSE_POLL, Steps};
use sbxm::github::Issue;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::jobs;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;

/// The error a scripted phase fails with.
type Failure = fn() -> anyhow::Error;

/// Plays `task run`, `task resume` and `task finish` for the loop, recording each call; a call
/// named in `fail` fails, leaving the task at `working`.
struct Script {
    base: PathBuf,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    /// The calls that fail, with the error each gives, every time it is made.
    fail: RefCell<Vec<(String, Failure)>>,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            calls: RefCell::default(),
            clock: Cell::new(START),
            fail: RefCell::default(),
        }
    }

    fn failing(self, call: &str, error: Failure) -> Self {
        self.fail.borrow_mut().push((call.to_owned(), error));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn phase(&self, call: String, issue: u32) -> Result<()> {
        self.clock.set(self.clock.get() + 60);
        self.calls.borrow_mut().push(call.clone());
        if let Some((_, error)) = self.fail.borrow().iter().find(|(c, _)| *c == call) {
            write_record(&self.base, issue, Stage::Working, Status::Failed, None);
            return Err(error());
        }
        write_record(&self.base, issue, Stage::Ready, Status::Ok, None);
        Ok(())
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.phase(format!("run {issue}"), issue)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.phase(format!("resume {issue}"), issue)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("finish {issue}"));
        let mut rec = read_record(&self.base, issue);
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&record::task_dir(&self.base, &rec.id), &rec).unwrap();
        Ok(())
    }

    fn file_findings(
        &self,
        _issue: u32,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }

    fn fix_check(
        &self,
        pr: u32,
        run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr} from run {run}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        // A queued job's own runs while its file is in `running/`; the loop's before each step
        // isn't recorded.
        let running = jobs::running_dir(&self.base);
        if fs::read_dir(&running).is_ok_and(|mut entries| entries.next().is_some()) {
            self.calls.borrow_mut().push("rm-merged job".to_owned());
        }
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

fn read_record(base: &Path, issue: u32) -> Record {
    record::read(&record::task_dir(base, &format!("issue-{issue}")).join("task.json")).unwrap()
}

/// Writes `issue-<n>`'s record at `stage` and `status`, keeping one already there otherwise;
/// `process`: the process recorded as working on it, when given.
fn write_record(base: &Path, issue: u32, stage: Stage, status: Status, process: Option<Process>) {
    let id = format!("issue-{issue}");
    let dir = record::task_dir(base, &id);
    let mut rec = record::read(&dir.join("task.json")).unwrap_or_else(|_| {
        Record::new(
            &NewTask {
                kind: Kind::Issue,
                number: issue,
                repo: REPO,
                title: &format!("Fix {issue}"),
                base: "main",
                branch: &format!("sbxm/issue-{issue}"),
                config_hash: "h",
                id: None,
            },
            START,
            Process::new(1, 0),
        )
    });
    rec.stage = stage;
    rec.status = status;
    if stage == Stage::Ready {
        rec.review = Some(ReviewResult {
            round: 1,
            ..ReviewResult::default()
        });
    }
    if process.is_some() {
        rec.process = process;
    }
    record::write(&dir, &rec).unwrap();
}

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

fn fixture() -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\nusage_wait = \
         \"30m\"\nusage_wait_total = \"1h\"\n",
    )
    .unwrap();
    Fixture { env, repo_root }
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        }
    }

    fn waiting(&self) -> Options {
        Options {
            wait: true,
            ..self.opts()
        }
    }

    fn queue_job(&self, name: &str, text: &str) {
        let queue = jobs::queue_dir(&self.base());
        fs::create_dir_all(&queue).unwrap();
        fs::write(queue.join(name), text).unwrap();
    }
}

fn labeled(number: u32) -> Issue {
    open_issue(number, &[READY, "size:S"], "")
}

fn github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for issue in &issues {
        gh = gh.with_issue_text(issue_text(issue.number));
    }
    gh.with_open_issues(issues)
}

struct Run {
    result: Result<()>,
    out: String,
    sleeps: Vec<Duration>,
}

/// Runs the loop; before each sleep's return, `on_sleep` is given how many sleeps there were,
/// this one included; `stop` is asked as the user's stop request.
fn run_loop(
    f: &Fixture,
    opts: Options,
    script: &Script,
    gh: &FakeGitHub,
    probe: &dyn record::ProcessProbe,
    on_sleep: &dyn Fn(usize),
    stop: &dyn Fn() -> bool,
) -> Run {
    let sleeps = RefCell::new(Vec::new());
    let now = || script.clock.get();
    let sleep = |d: Duration| {
        sleeps.borrow_mut().push(d);
        script.clock.set(script.clock.get() + d.as_secs());
        let n = sleeps.borrow().len();
        on_sleep(n);
    };
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        &opts,
        script,
        gh,
        probe,
        &env,
        &mut out,
        &mut warn,
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
        sleeps: sleeps.into_inner(),
    }
}

// --- `--wait` ---

#[test]
fn waiting_takes_a_job_then_a_labelled_issue_then_stops_on_request() {
    let f = fixture();
    let gh = github(Vec::new()).with_issue_text(issue_text(7));
    let script = Script::new(&f.base());
    let stop = Cell::new(false);
    let on_sleep = |n: usize| match n {
        1 => f.queue_job("a.json", r#"{"kind": "rm-merged"}"#),
        2 => {
            gh.set_open_issues(vec![labeled(7)]);
        }
        _ => stop.set(true),
    };

    let run = run_loop(&f, f.waiting(), &script, &gh, &Probe, &on_sleep, &|| {
        stop.get()
    });

    run.result.unwrap();
    assert_eq!(
        script.calls(),
        ["rm-merged job", "run 7", "finish 7"],
        "{}",
        run.out
    );
    assert_eq!(run.sleeps, [PAUSE_POLL; 3], "{}", run.out);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
    assert!(
        run.out.contains("checking again every 5m"),
        "says it waits: {}",
        run.out
    );
}

#[test]
fn without_wait_an_empty_queue_still_stops_the_loop() {
    let f = fixture();
    let gh = github(Vec::new());
    let script = Script::new(&f.base());

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    run.result.unwrap();
    assert!(run.sleeps.is_empty(), "{:?}", run.sleeps);
    assert!(run.out.contains("the queue is empty"), "{}", run.out);
}

#[test]
fn wait_with_a_list_of_issues_is_refused() {
    let f = fixture();
    let gh = github(vec![labeled(5)]);
    let script = Script::new(&f.base());
    let opts = Options {
        issues: vec![5],
        ..f.waiting()
    };

    let run = run_loop(&f, opts, &script, &gh, &Probe, &|_| {}, &|| false);

    let e = run.result.unwrap_err();
    assert!(format!("{e:#}").contains("--wait"), "{e:#}");
    assert!(script.calls().is_empty(), "{:?}", script.calls());
}

#[test]
fn the_cli_refuses_wait_with_issue() {
    let env = TestEnv::new();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "loop", "--wait", "--issue", "5"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--wait"), "{stderr}");
}

// --- `--stop` ---

/// Plants the loop lock of a loop whose process `Probe` reports alive.
fn plant_live_loop(base: &Path) {
    let path = sbxm::task::queue::lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_string(&Process::new(77, 0)).unwrap()).unwrap();
}

fn stop_loop(f: &Fixture, probe: &dyn record::ProcessProbe) -> (Result<()>, String) {
    let mut out = Vec::new();
    let result = task_loop::stop(&f.env.config_dir(), probe, &mut out);
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn stop_with_no_loop_running_says_so_and_writes_nothing() {
    let f = fixture();

    let (result, out) = stop_loop(&f, &Probe);

    result.unwrap();
    assert!(out.contains("no `sbxm task loop` is running"), "{out}");
    assert!(!task_loop::stop_path(&f.base()).exists());
}

#[test]
fn stop_with_a_dead_loop_lock_says_so_and_writes_nothing() {
    let f = fixture();
    plant_live_loop(&f.base());

    let (result, out) = stop_loop(&f, &Gone(77));

    result.unwrap();
    assert!(out.contains("no `sbxm task loop` is running"), "{out}");
    assert!(!task_loop::stop_path(&f.base()).exists());
}

#[test]
fn stop_with_a_loop_running_writes_the_request_next_to_its_lock() {
    let f = fixture();
    plant_live_loop(&f.base());

    let (result, out) = stop_loop(&f, &Probe);

    result.unwrap();
    assert!(out.contains("process 77"), "{out}");
    let path = task_loop::stop_path(&f.base());
    assert!(path.is_file());
    assert_eq!(
        path.parent(),
        sbxm::task::queue::lock_path(&f.base()).parent()
    );
}

#[test]
fn a_stop_request_ends_a_waiting_loop_within_one_interval_and_is_removed() {
    let f = fixture();
    let gh = github(Vec::new());
    let script = Script::new(&f.base());
    let on_sleep = |_| fs::write(task_loop::stop_path(&f.base()), "").unwrap();

    let run = run_loop(&f, f.waiting(), &script, &gh, &Probe, &on_sleep, &|| false);

    run.result.unwrap();
    assert_eq!(run.sleeps, [PAUSE_POLL], "{}", run.out);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
    assert!(!task_loop::stop_path(&f.base()).exists());
}

#[test]
fn a_stop_request_during_a_task_stops_the_loop_after_it() {
    let f = fixture();
    let gh = github(vec![labeled(3), labeled(4)]);
    let script = Script::new(&f.base());
    let stop_file = task_loop::stop_path(&f.base());
    // Written while task 3 runs, as `task loop --stop` from another terminal would.
    let stop = || {
        if script.calls().iter().any(|c| c == "run 3") {
            let _ = fs::write(&stop_file, "");
        }
        false
    };

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &stop);

    run.result.unwrap();
    assert_eq!(script.calls(), ["run 3", "finish 3"], "{}", run.out);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
    assert!(!stop_file.exists());
}

#[test]
fn a_stale_stop_request_is_removed_at_start_and_not_obeyed() {
    let f = fixture();
    let stop_file = task_loop::stop_path(&f.base());
    fs::create_dir_all(stop_file.parent().unwrap()).unwrap();
    fs::write(&stop_file, "").unwrap();
    let gh = github(vec![labeled(3)]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    run.result.unwrap();
    assert_eq!(script.calls(), ["run 3", "finish 3"], "{}", run.out);
    assert!(!stop_file.exists());
    assert!(
        run.out.contains("stop request left from before"),
        "{}",
        run.out
    );
}

/// A probe for which process `.0` is gone and every other is alive.
struct Gone(u32);

impl record::ProcessProbe for Gone {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (pid != self.0).then_some(0)
    }
}

#[test]
fn the_cli_stop_with_no_loop_running_says_so_and_exits_0() {
    let env = TestEnv::new();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "loop", "--stop"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("no `sbxm task loop` is running"),
        "{stdout}"
    );
    assert!(!task_loop::stop_path(&env.base_dir()).exists());
}

// --- Exit codes ---

fn signed_out() -> anyhow::Error {
    sbxm::task::pipeline::NotSignedIn::new("Not authenticated to Docker")
        .then(vec!["sbxm task run --issue 3".to_owned()])
        .into()
}

fn no_secret() -> anyhow::Error {
    sbxm::task::pipeline::MissingSecret::new("the cloud has no global 'github' login").into()
}

fn usage_limit() -> anyhow::Error {
    anyhow!("worker claude failed: usage limit reached")
}

fn boom() -> anyhow::Error {
    anyhow!("boom")
}

fn exit_code(run: &Run) -> i32 {
    task_loop::exit_code(&run.result)
}

#[test]
fn a_lapsed_sign_in_exits_3() {
    let f = fixture();
    let gh = github(vec![labeled(3)]);
    let script = Script::new(&f.base()).failing("run 3", signed_out);

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(exit_code(&run), 3, "{:?}", run.result);
}

#[test]
fn a_missing_secret_exits_3() {
    let f = fixture();
    let gh = github(vec![labeled(3)]);
    let script = Script::new(&f.base()).failing("run 3", no_secret);

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(exit_code(&run), 3, "{:?}", run.result);
    let e = run.result.unwrap_err();
    assert!(
        format!("{e:#}").contains("no global 'github' login"),
        "{e:#}"
    );
}

#[test]
fn two_parked_tasks_in_a_row_exit_3() {
    let f = fixture();
    let gh = github(vec![labeled(3), labeled(4)]);
    let script = Script::new(&f.base())
        .failing("run 3", boom)
        .failing("resume 3", boom)
        .failing("run 4", boom)
        .failing("resume 4", boom);

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(exit_code(&run), 3, "{:?}", run.result);
    let e = run.result.unwrap_err();
    assert!(format!("{e:#}").contains("two tasks parked"), "{e:#}");
}

#[test]
fn the_usage_waits_total_exits_3() {
    let f = fixture();
    let gh = github(vec![labeled(3)]);
    let script = Script::new(&f.base())
        .failing("run 3", usage_limit)
        .failing("resume 3", usage_limit);

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(exit_code(&run), 3, "{:?}", run.result);
    let e = run.result.unwrap_err();
    assert!(format!("{e:#}").contains("usage limit held"), "{e:#}");
}

#[test]
fn a_queued_run_jobs_lapsed_sign_in_exits_3() {
    let f = fixture();
    let gh = github(vec![labeled(3)]);
    f.queue_job("a.json", r#"{"kind": "run", "issue": 3}"#);
    let script = Script::new(&f.base()).failing("run 3", signed_out);

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(script.calls(), ["run 3"], "{}", run.out);
    assert_eq!(exit_code(&run), 3, "{:?}", run.result);
}

#[test]
fn another_error_exits_1() {
    let f = fixture();
    let gh = github(vec![labeled(3)]).failing("GitHub is down");
    let script = Script::new(&f.base());

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| false);

    assert_eq!(exit_code(&run), 1, "{:?}", run.result);
}

#[test]
fn a_stop_request_exits_0() {
    let f = fixture();
    let gh = github(vec![labeled(3)]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, f.opts(), &script, &gh, &Probe, &|_| {}, &|| true);

    assert_eq!(exit_code(&run), 0, "{:?}", run.result);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
}
