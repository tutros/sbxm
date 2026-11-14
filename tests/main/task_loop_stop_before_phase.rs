//! Review of issue 570: a stop the loop sees as it starts a task's phase (at the sandbox slot's
//! wait, `slot::Stopped`, or at the disk pause before it) is a requested stop, never a parked task:
//! it doesn't count toward the two-parked limit (exit 0, not 3), and a stop before a task's first
//! phase ran leaves that task as it was, a cut-off task's resume still owed to the next loop.
//! The phases are scripted; a phase scripted to stop asks the stop and fails with
//! `slot::Stopped`, as the real slot wait does.

use crate::common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::Issue;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

const REPO: &str = "o/r";
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;
/// The process of the loop that died, as its lock and its tasks name it.
const DEAD_LOOP: u32 = 77;

/// Every process is alive but the dead loop's.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (pid != DEAD_LOOP).then_some(0)
    }
}

/// What a scripted phase does.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// Reaches `ready`.
    Ready,
    /// Fails, leaving the task at `working`.
    Fail,
    /// The stop comes in while it waits for the slot: it asks the stop and fails with
    /// `slot::Stopped`, having written nothing.
    StopAtSlot,
}

struct Script {
    base: PathBuf,
    calls: RefCell<Vec<String>>,
    acts: RefCell<Vec<(String, Act)>>,
    clock: Cell<u64>,
    stop: Cell<bool>,
    /// `room()` fails while this is set: the disk is short.
    short: Cell<bool>,
    /// The call after which the disk is short.
    short_after: RefCell<Option<String>>,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            calls: RefCell::default(),
            acts: RefCell::default(),
            clock: Cell::new(START),
            stop: Cell::new(false),
            short: Cell::new(false),
            short_after: RefCell::default(),
        }
    }

    fn short_after(self, call: &str) -> Self {
        *self.short_after.borrow_mut() = Some(call.to_owned());
        self
    }

    fn with(self, call: &str, act: Act) -> Self {
        self.acts.borrow_mut().push((call.to_owned(), act));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn phase(&self, call: String, issue: u32) -> Result<()> {
        self.clock.set(self.clock.get() + 60);
        let act = self
            .acts
            .borrow()
            .iter()
            .find(|(c, _)| *c == call)
            .map_or(Act::Ready, |(_, act)| *act);
        if act == Act::StopAtSlot {
            self.stop.set(true);
            return Err(sbxm::slot::Stopped.into());
        }
        if self.short_after.borrow().as_deref() == Some(call.as_str()) {
            self.short.set(true);
        }
        self.calls.borrow_mut().push(call);
        if act == Act::Fail {
            write_record(&self.base, issue, Stage::Working, Status::Failed, None);
            return Err(anyhow!("boom"));
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

    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }

    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }

    fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        if self.short.get() {
            return Err(anyhow!("the disk is short"));
        }
        Ok(())
    }
}

fn read_record(base: &Path, issue: u32) -> Record {
    record::read(&record::task_dir(base, &format!("issue-{issue}")).join("task.json")).unwrap()
}

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
        rec.review = Some(record::ReviewResult {
            round: 1,
            ..record::ReviewResult::default()
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
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    Fixture { env, repo_root }
}

fn labeled(number: u32) -> Issue {
    open_issue(number, &["ready-for-worker", "size:S"], "")
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
}

/// One loop; at each sleep `on_sleep` runs with the script.
fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub, on_sleep: &dyn Fn(&Script)) -> Run {
    let now = || script.clock.get();
    let sleep = |d: Duration| {
        script.clock.set(script.clock.get() + d.as_secs());
        on_sleep(script);
    };
    let stop = || script.stop.get();
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        script,
        gh,
        &Gone,
        &env,
        &mut out,
        &mut warn,
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
    }
}

fn assert_stopped_on_request(run: &Run) {
    assert_eq!(
        task_loop::exit_code(&run.result),
        0,
        "{:?}\n{}",
        run.result,
        run.out
    );
    assert!(
        run.out.lines().any(|l| l == "stopped on request"),
        "{}",
        run.out
    );
    assert!(!run.out.contains("two tasks parked"), "{}", run.out);
}

/// `issue-<n>` at `working`, running in the dead loop's process when it died.
fn cut_off(base: &Path, issue: u32) -> PathBuf {
    write_record(
        base,
        issue,
        Stage::Working,
        Status::Running,
        Some(Process::new(DEAD_LOOP, 0)),
    );
    let lock = sbxm::task::queue::lock_path(base);
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(
        &lock,
        serde_json::to_string(&Process::new(DEAD_LOOP, 0)).unwrap(),
    )
    .unwrap();
    record::task_dir(base, &format!("issue-{issue}"))
}

fn resumed_hook(dir: &Path) -> serde_json::Value {
    record::read(&dir.join("task.json")).unwrap().hooks["task_loop_resumed"].clone()
}

// --- M-1: a stop is never a parked task ---

#[test]
fn a_stop_at_the_slot_after_a_parked_task_exits_0_not_3() {
    let f = fixture();
    let gh = github(vec![labeled(3), labeled(4)]);
    let script = Script::new(&f.base())
        .with("run 3", Act::Fail)
        .with("resume 3", Act::Fail)
        .with("run 4", Act::Fail)
        .with("resume 4", Act::StopAtSlot);

    let run = run_loop(&f, &script, &gh, &|_| {});

    assert_stopped_on_request(&run);
    assert_eq!(
        script.calls(),
        ["run 3", "resume 3", "run 4"],
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("stopped on request; continue it with `sbxm task resume --issue 4`"),
        "{}",
        run.out
    );
}

#[test]
fn a_stop_at_the_disk_pause_after_a_parked_task_exits_0_not_3() {
    let f = fixture();
    let gh = github(vec![labeled(3), labeled(4)]);
    // Issue 4's run fails, and the disk runs short before its resume; the stop comes in the pause.
    let script = Script::new(&f.base())
        .with("run 3", Act::Fail)
        .with("resume 3", Act::Fail)
        .with("run 4", Act::Fail)
        .short_after("run 4");

    let run = run_loop(&f, &script, &gh, &|s| s.stop.set(true));

    assert_stopped_on_request(&run);
    assert_eq!(
        script.calls(),
        ["run 3", "resume 3", "run 4"],
        "{}",
        run.out
    );
}

// --- M-2: a stop before a cut-off task's resume ran leaves it owed ---

#[test]
fn a_stop_at_the_slot_of_a_cut_off_tasks_resume_leaves_it_owed_to_the_next_loop() {
    let f = fixture();
    let dir = cut_off(&f.base(), 3);
    // Marked owed by the loop as it starts, for the dead loop's process.
    let owed = serde_json::to_value(Process::new(DEAD_LOOP, 0)).unwrap();
    let gh = github(Vec::new());
    let script = Script::new(&f.base()).with("resume 3", Act::StopAtSlot);

    let run = run_loop(&f, &script, &gh, &|_| {});

    assert_stopped_on_request(&run);
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert_eq!(resumed_hook(&dir), owed, "still owed: {}", run.out);
    assert!(
        run.out
            .contains("#3 (Fix 3) was cut off when a loop stopped and not resumed yet"),
        "{}",
        run.out
    );
    assert!(
        !run.out.contains("stopped on request before its resume"),
        "no row for a phase that never ran: {}",
        run.out
    );

    // The next loop, with no stop, resumes and finishes it, once.
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &gh, &|_| {});

    run.result.unwrap();
    assert_eq!(script.calls(), ["resume 3", "finish 3"], "{}", run.out);
    assert_eq!(resumed_hook(&dir), serde_json::json!(true));
}

#[test]
fn a_stop_at_the_disk_pause_before_a_cut_off_tasks_resume_leaves_it_owed() {
    let f = fixture();
    let dir = cut_off(&f.base(), 3);
    let owed = serde_json::to_value(Process::new(DEAD_LOOP, 0)).unwrap();
    let gh = github(Vec::new());
    let script = Script::new(&f.base());
    script.short.set(true);

    let run = run_loop(&f, &script, &gh, &|s| s.stop.set(true));

    assert_stopped_on_request(&run);
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert_eq!(resumed_hook(&dir), owed, "still owed: {}", run.out);
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }
}
