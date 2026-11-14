//! Issue 616 (decision 322): a task the loop's child runs records the loop that owns it, as
//! `loop: {pid, started_at}` next to its own `process`; `sbxm status` and the loop's restart
//! recovery match a task to its loop by `loop` when it is there, else by `process` as before.
//! The stamp itself (a child of a loop writing `loop`) is in `task_loop_owner_stamp.rs`: it sets
//! this process's loop, which no test here may see.

use crate::common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::Probe;
use sbxm::commands::task_loop::child::{Child, Exit, Spawner};
use sbxm::commands::task_loop::{self, Env, Options, Steps, TaskSteps};
use sbxm::github::fake::FakeGitHub;
use sbxm::status::looping;
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, ReviewResult, Stage, Status,
};

const REPO: &str = "o/r";
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;
/// The loop: the process its lock names.
const LOOP: u32 = 77;
/// The loop's child running the task.
const CHILD: u32 = 88;
/// A command run by hand.
const HAND: u32 = 99;
/// Another loop.
const OTHER_LOOP: u32 = 66;

fn process(pid: u32) -> Process {
    Process::new(pid, 0)
}

fn new_record(issue: u32) -> Record {
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
        process(1),
    )
}

/// `issue-<n>` running at `stage` in `process`, owned by `owner` when given.
fn save(base: &Path, issue: u32, stage: Stage, process: u32, owner: Option<u32>) -> PathBuf {
    let mut rec = new_record(issue);
    rec.stage = stage;
    rec.status = Status::Running;
    rec.process = Some(self::process(process));
    rec.loop_owner = owner.map(self::process);
    let dir = record::task_dir(base, &rec.id);
    record::write(&dir, &rec).unwrap();
    dir
}

// --- The field ---

#[test]
fn an_old_record_without_loop_reads_as_none_and_a_task_by_hand_writes_no_loop() {
    let rec = new_record(3);
    assert_eq!(rec.loop_owner, None);
    let json = serde_json::to_value(&rec).unwrap();
    assert!(json.get("loop").is_none(), "{json}");
    let read: Record = serde_json::from_value(json).unwrap();
    assert_eq!(read.loop_owner, None);
}

#[test]
fn the_loop_is_written_as_loop_with_its_pid_and_start() {
    let mut rec = new_record(3);
    rec.loop_owner = Some(Process::new(LOOP, START));

    let json = serde_json::to_value(&rec).unwrap();

    assert_eq!(
        json["loop"],
        serde_json::json!({"pid": LOOP, "started_at": "2026-11-02T10:00:00Z"})
    );
    let read: Record = serde_json::from_value(json).unwrap();
    assert_eq!(read.loop_owner, Some(Process::new(LOOP, START)));
}

#[test]
fn the_loop_survives_the_childs_own_record_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rec = new_record(3);
    rec.process = Some(process(CHILD));
    rec.loop_owner = Some(process(LOOP));
    let child = || process(CHILD);

    rec.advance(Stage::Working, START, child()).unwrap();
    rec.finish(Status::Failed).unwrap();
    rec.rerun(START, child()).unwrap();
    rec.finish(Status::Completed).unwrap();
    rec.begin_gating(START, child()).unwrap();
    rec.finish(Status::Passed).unwrap();
    rec.begin_review(START, child()).unwrap();
    rec.finish_at(Status::Completed, START).unwrap();
    rec.advance(Stage::Ready, START, child()).unwrap();
    record::write(tmp.path(), &rec).unwrap();
    let read = record::read(&tmp.path().join("task.json")).unwrap();

    assert_eq!(read.stage, Stage::Ready);
    assert_eq!(read.process, Some(process(CHILD)));
    assert_eq!(read.loop_owner, Some(process(LOOP)));
}

#[test]
fn a_command_by_hand_that_takes_the_task_drops_the_loop() {
    let mut rec = new_record(3);
    rec.stage = Stage::Working;
    rec.status = Status::Failed;
    rec.process = Some(process(CHILD));
    rec.loop_owner = Some(process(LOOP));

    rec.rerun(START, process(HAND)).unwrap();

    assert_eq!(rec.process, Some(process(HAND)));
    assert_eq!(rec.loop_owner, None);
}

#[test]
fn the_loops_identity_reads_from_its_json_and_nothing_else() {
    let json = serde_json::to_string(&Process::new(LOOP, START)).unwrap();
    assert_eq!(record::parse_loop(&json), Some(Process::new(LOOP, START)));
    assert_eq!(record::parse_loop(""), None);
    assert_eq!(record::parse_loop("77"), None);
    assert_eq!(record::parse_loop("{\"pid\": 77}"), None);
}

// --- The loop names itself to its child ---

/// Keeps each child it is given and says it failed, writing nothing.
#[derive(Default)]
struct Kept(RefCell<Vec<Child>>);

impl Spawner for Kept {
    fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
        self.0.borrow_mut().push(child.clone());
        Ok(Exit {
            success: false,
            how: "exited with code 1".into(),
            errors: String::new(),
        })
    }

    fn stop(&self) {}
}

#[test]
fn each_child_is_given_the_loops_own_process() {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    let gh = FakeGitHub::default();
    let backend = sbxm::backend::FakeBackend::default();
    let host = sbxm::task::gates::FakeHostRunner::default();
    let spawner = Kept::default();
    let config_dir = env.config_dir();
    let steps = TaskSteps {
        config_dir: &config_dir,
        repo_root,
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
        spawner: &spawner,
    };

    steps.run_task(3, None, &mut Vec::new(), &mut Vec::new());
    steps.resume_task(3, &mut Vec::new(), &mut Vec::new());
    steps.should_fix_task(3, &mut Vec::new(), &mut Vec::new());

    let me = serde_json::to_string(&Process::current(&Probe)).unwrap();
    let started = spawner.0.borrow();
    assert_eq!(started.len(), 3);
    for child in started.iter() {
        assert!(
            child
                .env
                .contains(&(record::LOOP_ENV.to_owned(), me.clone())),
            "{:?}",
            child.env
        );
    }
}

// --- `sbxm status` ---

/// A child of the loop that was killed: its record still says `running`.
const DEAD_CHILD: u32 = 55;

/// While the loop runs: only the killed child is gone.
const LOOP_ALIVE: Gone = Gone(&[DEAD_CHILD]);

fn current(base: &Path) -> Option<String> {
    looping::work(base, &process(LOOP), &LOOP_ALIVE)
        .unwrap()
        .current
        .map(|c| c.id)
}

#[test]
fn a_running_child_of_the_loop_is_the_loops_current_task() {
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, CHILD, Some(LOOP));

    assert_eq!(current(&env.base_dir()).as_deref(), Some("issue-3"));
}

#[test]
fn a_task_run_by_hand_while_a_loop_runs_is_not_the_loops() {
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, HAND, None);

    assert_eq!(current(&env.base_dir()), None);
}

#[test]
fn a_child_of_another_loop_is_not_this_loops() {
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, CHILD, Some(OTHER_LOOP));

    assert_eq!(current(&env.base_dir()), None);
}

#[test]
fn an_old_record_whose_process_is_the_loop_is_still_the_loops() {
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, LOOP, None);

    assert_eq!(current(&env.base_dir()).as_deref(), Some("issue-3"));
}

#[test]
fn a_child_of_another_loop_in_this_loops_process_is_not_this_loops() {
    // `loop` decides when it is there, even against an equal `process`.
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, LOOP, Some(OTHER_LOOP));

    assert_eq!(current(&env.base_dir()), None);
}

#[test]
fn a_killed_child_of_the_loop_does_not_hide_its_running_child() {
    // Review M-1: issue 9's child was killed and the task parked; the loop runs issue 3 now.
    let env = TestEnv::new();
    save(&env.base_dir(), 3, Stage::Working, CHILD, Some(LOOP));
    save(&env.base_dir(), 9, Stage::Working, DEAD_CHILD, Some(LOOP));

    assert_eq!(current(&env.base_dir()).as_deref(), Some("issue-3"));
}

#[test]
fn a_killed_child_alone_is_no_current_task() {
    let env = TestEnv::new();
    save(&env.base_dir(), 9, Stage::Working, DEAD_CHILD, Some(LOOP));

    assert_eq!(current(&env.base_dir()), None);
}

#[test]
fn a_task_its_child_released_is_no_current_task() {
    // The phase's own lock clears `process` as the child returns; `loop` stays.
    let env = TestEnv::new();
    let dir = save(&env.base_dir(), 9, Stage::Working, CHILD, Some(LOOP));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.process = None;
    record::write(&dir, &rec).unwrap();

    assert_eq!(current(&env.base_dir()), None);
}

#[test]
fn a_task_whose_lock_a_live_command_holds_is_still_the_loops() {
    // Its recorded process is gone, but a live command holds its lock, as the Tasks section
    // counts it: not interrupted.
    let env = TestEnv::new();
    let dir = save(&env.base_dir(), 3, Stage::Working, DEAD_CHILD, Some(LOOP));
    let mut holder = serde_json::to_value(process(CHILD)).unwrap();
    holder["token"] = serde_json::json!("t");
    fs::write(dir.join("lock.json"), holder.to_string()).unwrap();

    assert_eq!(current(&env.base_dir()).as_deref(), Some("issue-3"));
}

// --- The loop's restart recovery (decision 310(4)) ---

/// A probe for which the processes in `.0` are gone and every other is alive.
struct Gone(&'static [u32]);

impl ProcessProbe for Gone {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (!self.0.contains(&pid)).then_some(0)
    }
}

/// The dead loop and its dead child; the other loop and the hand command are alive.
const GONE: Gone = Gone(&[LOOP, CHILD]);

/// Plays `task run`, `task resume` and `task finish`, recording each call; a resume of an issue
/// in `fail` fails, leaving the task at `working`.
struct Script {
    base: PathBuf,
    calls: RefCell<Vec<String>>,
    fail: Vec<u32>,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            calls: RefCell::default(),
            fail: Vec::new(),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn phase(&self, call: String, issue: u32) -> Result<()> {
        self.calls.borrow_mut().push(call);
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json")).unwrap_or_else(|_| new_record(issue));
        if self.fail.contains(&issue) {
            rec.stage = Stage::Working;
            rec.status = Status::Failed;
            record::write(&dir, &rec)?;
            return Err(anyhow!("boom"));
        }
        rec.stage = Stage::Ready;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            ..ReviewResult::default()
        });
        record::write(&dir, &rec)
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
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json"))?;
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec)
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
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

/// A loop's checkout, and the lock of the loop of process 77 left behind.
fn fixture() -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let lock = sbxm::task::queue::lock_path(&env.base_dir());
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, serde_json::to_string(&process(LOOP)).unwrap()).unwrap();
    Fixture { env, repo_root }
}

/// Starts a loop over an empty queue: it only does what its restart recovery finds.
fn restart(f: &Fixture, script: &Script) -> String {
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_open_issues(Vec::new());
    let clock = std::cell::Cell::new(START);
    let env = Env {
        now: &|| clock.get(),
        sleep: &|d: Duration| clock.set(clock.get() + d.as_secs()),
        stop_requested: &|| false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        script,
        &gh,
        &GONE,
        &env,
        &mut out,
        &mut warn,
    )
    .unwrap();
    String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap()
}

#[test]
fn an_interrupted_child_of_the_dead_loop_is_resumed_once() {
    let f = fixture();
    let dir = save(&f.env.base_dir(), 3, Stage::Working, CHILD, Some(LOOP));
    let script = Script::new(&f.env.base_dir());

    let out = restart(&f, &script);

    assert_eq!(script.calls(), ["resume 3", "finish 3"], "{out}");
    assert!(out.contains("resuming it once"), "{out}");
    let rec = record::read(&dir.join("task.json")).unwrap();
    assert_eq!(rec.hooks["task_loop_resumed"], serde_json::json!(true));
}

#[test]
fn a_child_of_the_dead_loop_cut_off_again_after_its_resume_is_parked() {
    let f = fixture();
    let dir = save(&f.env.base_dir(), 3, Stage::Working, CHILD, Some(LOOP));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.hooks = serde_json::json!({ "task_loop_resumed": true });
    record::write(&dir, &rec).unwrap();
    let script = Script::new(&f.env.base_dir());

    let out = restart(&f, &script);

    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert!(
        out.contains("again after the loop resumed it once; parked"),
        "{out}"
    );
}

#[test]
fn a_resumed_child_that_fails_again_is_parked() {
    let f = fixture();
    save(&f.env.base_dir(), 3, Stage::Working, CHILD, Some(LOOP));
    let mut script = Script::new(&f.env.base_dir());
    script.fail.push(3);

    let out = restart(&f, &script);

    assert_eq!(script.calls(), ["resume 3"], "{out}");
    assert!(out.contains("parked at working"), "{out}");
}

#[test]
fn an_interrupted_task_run_by_hand_is_never_resumed_by_the_loop() {
    let f = fixture();
    // The hand command died too.
    save(&f.env.base_dir(), 3, Stage::Working, CHILD, None);
    let script = Script::new(&f.env.base_dir());

    let out = restart(&f, &script);

    assert!(script.calls().is_empty(), "{out}");
}

#[test]
fn a_child_of_another_live_loop_is_not_touched() {
    let f = fixture();
    let dir = save(
        &f.env.base_dir(),
        3,
        Stage::Working,
        CHILD,
        Some(OTHER_LOOP),
    );
    let before = fs::read_to_string(dir.join("task.json")).unwrap();
    let script = Script::new(&f.env.base_dir());

    let out = restart(&f, &script);

    assert!(script.calls().is_empty(), "{out}");
    assert_eq!(fs::read_to_string(dir.join("task.json")).unwrap(), before);
}

#[test]
fn an_old_record_whose_process_is_the_dead_loop_is_resumed_as_before() {
    let f = fixture();
    save(&f.env.base_dir(), 3, Stage::Working, LOOP, None);
    let script = Script::new(&f.env.base_dir());

    let out = restart(&f, &script);

    assert_eq!(script.calls(), ["resume 3", "finish 3"], "{out}");
}
