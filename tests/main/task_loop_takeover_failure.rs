//! Issue 710: a loop that took over a dead loop's lock keeps that loop in its own (`took_over`)
//! until it has marked the dead loop's cut-off tasks owed and named its adopted children. A loop
//! that fails (or is killed) in between leaves the next loop able to resume the cut-off task and
//! adopt the live child, as if it had taken the dead lock over itself.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use sbxm::backend::Location;
use sbxm::commands::task_loop::{
    self, Env, Options, Phase, PhaseEnd, RESUMED_HOOK, Started, Steps,
};
use sbxm::github::fake::FakeGitHub;
use sbxm::status::looping;
use sbxm::task::loop_log;
use sbxm::task::outcome::{self, Kind, Outcome};
use sbxm::task::queue::{AdoptedChild, LockFile, lock_path};
use sbxm::task::record::{
    self, NewTask, Process, ProcessProbe, Record, ReviewResult, Stage, Status,
};

use crate::common;

const REPO: &str = "o/r";
/// 2026-11-02T10:00:00Z: the loop that fails starts.
const START: u64 = 1_793_613_600;
/// The next loop starts later.
const RESTART: u64 = START + 600;
const TASK_START: u64 = START - 3_600;
/// The loop that died first, whose lock the failing loop takes over.
const DEAD_LOOP: u32 = 77;
/// A loop that took [`DEAD_LOOP`]'s lock over and died too, before it marked anything.
const SECOND_LOOP: u32 = 78;
/// The dead loop's child that died with it: its task was cut off.
const GONE_CHILD: u32 = 87;
/// The dead loop's child that outlived it.
const CHILD: u32 = 88;
/// The cut-off task's issue and the live child's.
const CUT: u32 = 3;
const LIVE: u32 = 4;
/// When the live child ends, after the next loop starts.
const CHILD_ENDS: u64 = RESTART + 10;

/// Every pid is alive (started at 0) except the dead loops, the gone child, and the live child
/// once the clock reaches [`CHILD_ENDS`], when it leaves its record at `ready` with its outcome.
/// This process started at `me`: each loop run here is told apart from the one before by it.
struct Probe<'a> {
    clock: &'a AtomicU64,
    base: PathBuf,
    me: u64,
    ended: Mutex<bool>,
}

impl ProcessProbe for Probe<'_> {
    fn start_time(&self, pid: u32) -> Option<u64> {
        if pid == std::process::id() {
            return Some(self.me);
        }
        if [DEAD_LOOP, SECOND_LOOP, GONE_CHILD].contains(&pid) {
            return None;
        }
        if pid == CHILD {
            let mut ended = self.ended.lock().unwrap();
            if *ended {
                return None;
            }
            if self.clock.load(Ordering::SeqCst) >= CHILD_ENDS {
                *ended = true;
                set_ready(&self.base, LIVE);
                let dir = record::task_dir(&self.base, &format!("issue-{LIVE}"));
                outcome::write(&dir, &Outcome::new(Kind::Ready, None, "")).unwrap();
                return None;
            }
        }
        Some(0)
    }
}

fn set_ready(base: &Path, issue: u32) {
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.process = None;
    rec.review = Some(ReviewResult {
        round: 1,
        must_fix: 0,
        not_run: Some(Vec::new()),
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
}

/// Issue `issue`'s task at `working` in the cloud, run by `pid` for [`DEAD_LOOP`].
fn running(base: &Path, issue: u32, pid: u32) {
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = Record::new(
        &NewTask {
            kind: record::Kind::Issue,
            number: issue,
            repo: REPO,
            title: &format!("Fix {issue}"),
            base: "main",
            branch: &format!("sbxm/issue-{issue}"),
            config_hash: "h",
            id: None,
        },
        TASK_START,
        Process::new(pid, 0),
    );
    rec.location = Location::Cloud;
    rec.stage = Stage::Working;
    rec.status = Status::Running;
    rec.loop_owner = Some(Process::new(DEAD_LOOP, 0));
    record::write(&dir, &rec).unwrap();
}

fn read(base: &Path, issue: u32) -> Record {
    record::read(&record::task_dir(base, &format!("issue-{issue}")).join("task.json")).unwrap()
}

/// Plays the loop's phases: each ends at once, ready.
struct Play<'a> {
    base: PathBuf,
    events: &'a Mutex<Vec<String>>,
}

impl Play<'_> {
    fn event(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }

    fn ready(&self, issue: u32, what: &str) {
        let at = read(&self.base, issue).location;
        self.event(format!("start {issue} {what} {}", at.as_str()));
        set_ready(&self.base, issue);
    }
}

struct Done;

impl Started for Done {
    fn poll(&mut self, _out: &mut dyn Write) -> Option<PhaseEnd> {
        Some(PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, "")))
    }
}

impl Steps for Play<'_> {
    fn start_task(
        &self,
        issue: u32,
        phase: Phase,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Box<dyn Started + '_> {
        let what = match phase {
            Phase::Run(_) => "run",
            Phase::Resume => "resume",
            Phase::ShouldFix => "should-fix",
        };
        self.ready(issue, what);
        Box::new(Done)
    }

    fn run_at(
        &self,
        issue: u32,
        _location: Option<Location>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected run of #{issue}"))
    }

    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.run_at(issue, None, out, warn)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.ready(issue, "resume");
        Ok(())
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.event(format!("finish {issue}"));
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json"))?;
        rec.stage = Stage::Finished;
        rec.status = Status::Ok;
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
        _run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

struct Fixture {
    env: common::Env,
    repo_root: PathBuf,
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }
}

/// One local slot and `cloud_slots`; [`DEAD_LOOP`]'s lock is left, with its cut-off task
/// ([`CUT`]) and its live child ([`LIVE`]).
fn fixture(cloud_slots: Option<u32>) -> Fixture {
    let env = common::Env::new();
    if let Some(n) = cloud_slots {
        common::cloud_slot::set_cloud_slots(&env, n);
    }
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let base = env.base_dir();
    let lock = lock_path(&base);
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, serde_json::to_string(&dead_lock()).unwrap()).unwrap();
    running(&base, CUT, GONE_CHILD);
    running(&base, LIVE, CHILD);
    Fixture { env, repo_root }
}

fn bare_lock(process: Process) -> LockFile {
    LockFile {
        process,
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: Vec::new(),
        idle_check: None,
        took_over: Vec::new(),
    }
}

fn dead_lock() -> LockFile {
    bare_lock(Process::new(DEAD_LOOP, 0))
}

/// GitHub with no issues waiting; the PRs of the loop's tasks (`100 + issue`) are armed, so they
/// await no one.
fn github() -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", "0123456789abcdef0123456789abcdef01234567");
    for n in [100 + CUT, 100 + LIVE] {
        gh = gh
            .with_pr(sbxm::github::PrInfo {
                number: n,
                head_ref: format!("sbxm/pr-{n}"),
                head_oid: "0123456789abcdef0123456789abcdef01234567".into(),
                is_cross_repository: false,
                head_repo: String::new(),
                state: sbxm::github::PrState::Open,
                title: format!("PR {n}"),
                body: String::new(),
                closing_issues: Vec::new(),
                base_moved: false,
            })
            .with_auto_merge(n);
    }
    gh.with_open_issues(Vec::new())
}

struct Run {
    result: Result<()>,
    out: String,
    events: Vec<String>,
    /// The loop's lock as the loop's first sleep saw it.
    lock: Option<String>,
}

impl Run {
    fn at(&self, event: &str) -> usize {
        self.events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no {event:?} in {:?}\n{}", self.events, self.out))
    }
}

/// Runs a loop started at `start` by this process started at `me`, to its stop; with `advance`
/// its sleeps move the clock.
fn run_loop(f: &Fixture, start: u64, me: u64, advance: bool) -> Run {
    let events = Mutex::new(Vec::new());
    let clock = AtomicU64::new(start);
    let probe = Probe {
        clock: &clock,
        base: f.base(),
        me,
        ended: Mutex::default(),
    };
    let play = Play {
        base: f.base(),
        events: &events,
    };
    let lock = RefCell::new(None);
    let lock_file = lock_path(&f.base());
    let env = Env {
        now: &|| clock.load(Ordering::SeqCst),
        sleep: &|d: Duration| {
            if lock.borrow().is_none() {
                *lock.borrow_mut() = fs::read_to_string(&lock_file).ok();
            }
            if advance {
                clock.fetch_add(d.as_secs(), Ordering::SeqCst);
            }
        },
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let mut warn = Vec::new();
    let result = task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        &play,
        &github(),
        &probe,
        &env,
        &mut out,
        &mut warn,
    );
    let events = events.lock().unwrap().clone();
    Run {
        result,
        out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
        events,
        lock: lock.into_inner(),
    }
}

/// The next loop resumes the cut-off task once and adopts the live child, finishing both, and
/// its lock names the adopted child with nothing taken over left in it.
fn assert_recovered(f: &Fixture, run: &Run) {
    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(
        run.out.contains(&format!("#{CUT}")) && run.out.contains("resuming it once"),
        "{}",
        run.out
    );
    run.at(&format!("start {CUT} resume cloud"));
    run.at(&format!("finish {CUT}"));
    assert_eq!(
        read(&f.base(), CUT).hooks[RESUMED_HOOK],
        serde_json::json!(true)
    );
    run.at(&format!("finish {LIVE}"));
    assert!(
        !run.events.contains(&format!("start {LIVE} resume cloud")),
        "{:?}\n{}",
        run.events,
        run.out
    );
    let lock: LockFile = serde_json::from_str(run.lock.as_deref().expect("a lock")).unwrap();
    assert_eq!(
        lock.adopted,
        [AdoptedChild {
            issue: LIVE,
            owner: Process::new(DEAD_LOOP, 0),
        }]
    );
    assert!(lock.took_over.is_empty(), "{:?}", lock.took_over);
}

// --- acceptance criterion 1: the loop that took over fails at `free_log` ---

#[test]
fn task_loop_takeover_failure_at_free_log_leaves_the_dead_loops_tasks_to_the_next_loop() {
    for cloud_slots in [Some(2), None] {
        let f = fixture(cloud_slots);
        // Every log name of the failing loop's start is taken, and its sleeps don't move the
        // clock: `free_log` fails, after the take-over and before any mark.
        let taken = loop_log::path(&f.base(), START);
        fs::create_dir_all(taken.parent().unwrap()).unwrap();
        fs::write(&taken, "").unwrap();

        let failed = run_loop(&f, START, 1, false);

        let e = failed.result.expect_err("free_log fails");
        assert!(format!("{e:#}").contains("log name"), "{e:#}");
        // The failing loop's lock is kept, with the dead loop's in it.
        let left: LockFile =
            serde_json::from_str(&fs::read_to_string(lock_path(&f.base())).unwrap()).unwrap();
        assert_eq!(left.process, Process::new(std::process::id(), 1));
        assert_eq!(left.took_over, dead_lock().taken_over());
        fs::remove_file(&taken).unwrap();

        let run = run_loop(&f, RESTART, 2, true);

        assert_recovered(&f, &run);
        // Its lock is removed as it stops.
        assert!(!lock_path(&f.base()).exists(), "{cloud_slots:?}");
    }
}

// --- acceptance criterion 2: the loop that took over is killed at the same point ---

#[test]
fn task_loop_takeover_failure_killed_after_the_take_over_leaves_the_dead_loops_tasks_to_the_next() {
    for cloud_slots in [Some(2), None] {
        let f = fixture(cloud_slots);
        // The lock as the killed loop's take-over wrote it.
        let mut killed = bare_lock(Process::new(SECOND_LOOP, 0));
        killed.took_over = dead_lock().taken_over();
        fs::write(
            lock_path(&f.base()),
            serde_json::to_string(&killed).unwrap(),
        )
        .unwrap();

        let run = run_loop(&f, RESTART, 2, true);

        assert_recovered(&f, &run);
    }
}

#[test]
fn task_loop_takeover_failure_two_loops_killed_in_a_row_leave_the_first_ones_tasks_to_the_next() {
    let f = fixture(Some(2));
    let mut second = bare_lock(Process::new(SECOND_LOOP, 0));
    second.took_over = dead_lock().taken_over();
    // A third loop, this process before a restart, killed after taking the second's lock over.
    let mut third = bare_lock(Process::new(std::process::id(), 1));
    third.took_over = second.taken_over();
    fs::write(lock_path(&f.base()), serde_json::to_string(&third).unwrap()).unwrap();

    let run = run_loop(&f, RESTART, 2, true);

    assert_recovered(&f, &run);
}

// --- `sbxm status` reads a lock with a loop it took over ---

#[test]
fn task_loop_takeover_failure_status_reads_the_lock_and_lists_the_first_loops_live_child() {
    let f = fixture(Some(2));
    let mut killed = bare_lock(Process::new(SECOND_LOOP, 0));
    killed.took_over = dead_lock().taken_over();
    fs::write(
        lock_path(&f.base()),
        serde_json::to_string(&killed).unwrap(),
    )
    .unwrap();
    let clock = AtomicU64::new(RESTART);
    let probe = Probe {
        clock: &clock,
        base: f.base(),
        me: 2,
        ended: Mutex::default(),
    };

    let (lock, file) = looping::read_lock_file(&f.base(), &probe);

    assert_eq!(lock, looping::Lock::Stale(Process::new(SECOND_LOOP, 0)));
    let file = file.expect("the lock parses");
    assert_eq!(file.took_over, dead_lock().taken_over());
    let dead = looping::dead(&f.base(), &file, &probe);
    let left: HashSet<&str> = dead.left.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(left, HashSet::from(["issue-4"]), "{:?}", dead.left);
}

// --- review M-1: many loops in a row fail before their marks ---

/// `failures` loops in a row take the lock over and fail before their marks (a folder at the stop
/// file's path fails each start); `sbxm status` reads the lock each leaves and names the first
/// loop's live child, and once the cause is gone the next loop recovers the first loop's tasks.
fn after_failures_in_a_row(failures: u64) {
    let f = fixture(Some(2));
    let stop = task_loop::stop_path(&f.base());
    fs::create_dir(&stop).unwrap();
    for attempt in 1..=failures {
        let run = run_loop(&f, START, attempt, false);
        let e = format!("{:#}", run.result.expect_err("the start fails"));
        assert!(e.contains("cannot remove"), "attempt {attempt}: {e}");
    }
    let clock = AtomicU64::new(START);
    let probe = Probe {
        clock: &clock,
        base: f.base(),
        me: failures + 1,
        ended: Mutex::default(),
    };
    let (_, file) = looping::read_lock_file(&f.base(), &probe);
    let file = file.unwrap_or_else(|| panic!("the lock parses after {failures} failures"));
    // One entry per loop taken over, the first loop first, none nested in another.
    assert_eq!(file.took_over.len() as u64, failures);
    assert_eq!(file.first(), &Process::new(DEAD_LOOP, 0));
    let dead = looping::dead(&f.base(), &file, &probe);
    let left: HashSet<&str> = dead.left.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(left, HashSet::from(["issue-4"]), "{:?}", dead.left);
    fs::remove_dir(&stop).unwrap();

    let run = run_loop(&f, RESTART, failures + 1, true);

    assert_recovered(&f, &run);
}

#[test]
fn task_loop_takeover_failure_126_failures_in_a_row_leave_the_first_loops_tasks_to_the_next() {
    after_failures_in_a_row(126);
}

#[test]
fn task_loop_takeover_failure_127_failures_in_a_row_leave_the_first_loops_tasks_to_the_next() {
    after_failures_in_a_row(127);
}

#[test]
fn task_loop_takeover_failure_128_failures_in_a_row_leave_the_first_loops_tasks_to_the_next() {
    after_failures_in_a_row(128);
}
