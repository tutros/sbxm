//! Issue 643 (milestone 5 slice 8, decision 317(12)): a restarted loop adopts the children that
//! outlived the loop whose lock it took over. Each counts against its slot and the 5-PR pause,
//! `sbxm status` lists it, and when it ends the loop reads its `outcome.json` and finishes it as
//! its own; one that ended with no outcome is resumed once under the rule of decision 310.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
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
use sbxm::github::{PrInfo, PrState};
use sbxm::status::looping;
use sbxm::task::outcome::{self, Kind, Outcome};
use sbxm::task::queue::{LockFile, lock_path};
use sbxm::task::record::{
    self, NewTask, Process, ProcessProbe, Record, ReviewResult, Stage, Status,
};

use crate::common;
use crate::common::task_fixture::{issue_text, open_issue};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// 2026-11-02T10:00:00Z: the new loop starts.
const START: u64 = 1_793_613_600;
/// 2026-11-02T09:00:00Z: the adopted task started, under the dead loop.
const TASK_START: u64 = START - 3_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// The loop that died, whose lock the new loop takes over.
const DEAD_LOOP: u32 = 77;
/// The dead loop's children that outlived it.
const CHILD: u32 = 88;
const CHILD_2: u32 = 89;
/// A command run by hand.
const HAND: u32 = 99;

/// How a phase ends.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// Reaches `ready`, clean, with its `outcome.json`.
    Ready,
    /// Killed: no outcome, the record left at `working`.
    Crash,
}

/// The processes: every pid is alive (started at 0) except the dead loop, the ones gone, and an
/// adopted child once the clock reaches its end, when it leaves its record and outcome as a real
/// child would.
struct Probe<'a> {
    clock: &'a AtomicU64,
    base: PathBuf,
    /// pid → (issue, when it ends, how).
    ends: HashMap<u32, (u32, u64, Act)>,
    gone: Mutex<HashSet<u32>>,
    events: &'a Mutex<Vec<String>>,
}

impl ProcessProbe for Probe<'_> {
    fn start_time(&self, pid: u32) -> Option<u64> {
        let mut gone = self.gone.lock().unwrap();
        if pid == DEAD_LOOP || gone.contains(&pid) {
            return None;
        }
        if let Some(&(issue, at, act)) = self.ends.get(&pid)
            && self.clock.load(Ordering::SeqCst) >= at
        {
            gone.insert(pid);
            write_record(&self.base, issue, act, None);
            if act == Act::Ready {
                let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
                outcome::write(&dir, &Outcome::new(Kind::Ready, None, "")).unwrap();
            }
            self.events
                .lock()
                .unwrap()
                .push(format!("end {issue} adopted"));
            return None;
        }
        Some(0)
    }
}

/// Issue `issue`'s record as a phase leaves it after `act`; a new record starts at
/// [`TASK_START`] in `location`.
fn write_record(base: &Path, issue: u32, act: Act, location: Option<Location>) {
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap_or_else(|_| {
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
            Process::new(1, 0),
        );
        rec.location = location.unwrap_or_default();
        rec
    });
    match act {
        Act::Ready => {
            rec.stage = Stage::Ready;
            rec.status = Status::Ok;
            rec.review = Some(ReviewResult {
                round: 1,
                must_fix: 0,
                not_run: Some(Vec::new()),
                ..ReviewResult::default()
            });
        }
        Act::Crash => {
            rec.stage = Stage::Working;
            rec.status = Status::Failed;
        }
    }
    record::write(&dir, &rec).unwrap();
}

/// Issue `issue`'s task at `working` in `location`, run by `process` for `owner`.
fn running(base: &Path, issue: u32, location: Location, process: Process, owner: Option<u32>) {
    write_record(base, issue, Act::Crash, Some(location));
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.status = Status::Running;
    rec.process = Some(process);
    rec.loop_owner = owner.map(|pid| Process::new(pid, 0));
    record::write(&dir, &rec).unwrap();
}

/// Issue `issue`'s task run by the dead loop's child `pid`, alive.
fn adoptable(base: &Path, issue: u32, location: Location, pid: u32) {
    running(base, issue, location, Process::new(pid, 0), Some(DEAD_LOOP));
}

fn read(base: &Path, issue: u32) -> Record {
    record::read(&record::task_dir(base, &format!("issue-{issue}")).join("task.json")).unwrap()
}

/// Plays the loop's own phases, each ending after its polls as the script says (default: at
/// once, ready).
struct Play<'a> {
    base: PathBuf,
    script: RefCell<HashMap<u32, VecDeque<(u32, Act)>>>,
    events: &'a Mutex<Vec<String>>,
}

impl<'a> Play<'a> {
    fn new(base: &Path, events: &'a Mutex<Vec<String>>) -> Self {
        Self {
            base: base.to_owned(),
            script: RefCell::default(),
            events,
        }
    }

    fn plays(self, issue: u32, phases: &[(u32, Act)]) -> Self {
        self.script
            .borrow_mut()
            .insert(issue, phases.iter().copied().collect());
        self
    }

    fn next(&self, issue: u32) -> (u32, Act) {
        self.script
            .borrow_mut()
            .get_mut(&issue)
            .and_then(VecDeque::pop_front)
            .unwrap_or((0, Act::Ready))
    }

    fn event(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }

    fn phase_end(act: Act) -> PhaseEnd {
        match act {
            Act::Ready => PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, "")),
            Act::Crash => PhaseEnd::Missing("the task's child sbxm was killed".into()),
        }
    }
}

struct Playing<'p, 'a> {
    play: &'p Play<'a>,
    issue: u32,
    polls: u32,
    act: Act,
}

impl Started for Playing<'_, '_> {
    fn poll(&mut self, _out: &mut dyn Write) -> Option<PhaseEnd> {
        if self.polls > 0 {
            self.polls -= 1;
            return None;
        }
        write_record(&self.play.base, self.issue, self.act, None);
        self.play
            .event(format!("end {} {:?}", self.issue, self.act));
        Some(Play::phase_end(self.act))
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
        let (what, location) = match phase {
            Phase::Run(location) => ("run", location.unwrap_or_default()),
            Phase::Resume => ("resume", read(&self.base, issue).location),
            Phase::ShouldFix => ("should-fix", read(&self.base, issue).location),
        };
        if let Phase::Run(_) = phase {
            write_record(&self.base, issue, Act::Crash, Some(location));
        }
        self.event(format!("start {issue} {what} {}", location.as_str()));
        let (polls, act) = self.next(issue);
        Box::new(Playing {
            play: self,
            issue,
            polls,
            act,
        })
    }

    // The loop with one task at a time runs each phase to its end here.
    fn run_at(
        &self,
        issue: u32,
        location: Option<Location>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        let at = location.unwrap_or_default();
        self.event(format!("start {issue} run {}", at.as_str()));
        let (_, act) = self.next(issue);
        write_record(&self.base, issue, act, Some(at));
        self.event(format!("end {issue} {act:?}"));
        match act {
            Act::Ready => Ok(()),
            Act::Crash => Err(anyhow!("played failure")),
        }
    }

    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.run_at(issue, None, out, warn)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let at = read(&self.base, issue).location;
        self.event(format!("start {issue} resume {}", at.as_str()));
        let (_, act) = self.next(issue);
        write_record(&self.base, issue, act, None);
        self.event(format!("end {issue} {act:?}"));
        match act {
            Act::Ready => Ok(()),
            Act::Crash => Err(anyhow!("played failure")),
        }
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

    fn log_text(&self) -> String {
        fs::read_to_string(
            self.base()
                .join(".sbxm/loop-logs/2026-11-02-100000-loop.md"),
        )
        .unwrap_or_default()
    }
}

/// One local slot and `cloud_slots` as given; with `dead_lock` the dead loop's lock is left.
fn fixture(cloud_slots: Option<u32>, dead_lock: bool) -> Fixture {
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
    if dead_lock {
        let lock = lock_path(&env.base_dir());
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            &lock,
            serde_json::to_string(&Process::new(DEAD_LOOP, 0)).unwrap(),
        )
        .unwrap();
    }
    Fixture { env, repo_root }
}

/// GitHub with these issues open and labeled ready, sized S; the PRs of the loop's tasks
/// (`100 + issue`) are armed, so they await no one.
fn github(issues: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    let mut open = Vec::new();
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n));
        open.push(open_issue(n, &[READY, "size:S"], "Why\n"));
    }
    for n in 101..=110 {
        gh = gh.with_pr(pr(n)).with_auto_merge(n);
    }
    gh.with_open_issues(open)
}

fn pr(number: u32) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("sbxm/pr-{number}"),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: format!("PR {number}"),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// A finished task of issue `issue` the loop opened PR `number` for, awaiting the user.
fn loop_pr(base: &Path, issue: u32, number: u32) {
    write_record(base, issue, Act::Ready, Some(Location::Local));
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{number}"));
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    record::write(&dir, &rec).unwrap();
}

struct Run {
    result: Result<()>,
    out: String,
    events: Vec<String>,
    /// The loop's lock as the loop's first sleep saw it.
    lock: Option<String>,
}

impl Run {
    /// The index of `event`; panics when it is not there.
    fn at(&self, event: &str) -> usize {
        self.events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no {event:?} in {:?}\n{}", self.events, self.out))
    }

    fn starts(&self) -> Vec<String> {
        self.events
            .iter()
            .filter(|e| e.starts_with("start"))
            .cloned()
            .collect()
    }
}

/// Runs the loop to its stop, with the children in `ends` (pid → issue, seconds after
/// [`START`] when it ends, how) ending as the clock moves with the loop's sleeps.
fn run_loop(
    f: &Fixture,
    gh: &FakeGitHub,
    ends: &[(u32, u32, u64, Act)],
    script: &[(u32, &[(u32, Act)])],
    stop: &dyn Fn() -> bool,
) -> Run {
    let events = Mutex::new(Vec::new());
    let clock = AtomicU64::new(START);
    let probe = Probe {
        clock: &clock,
        base: f.base(),
        ends: ends
            .iter()
            .map(|&(pid, issue, after, act)| (pid, (issue, START + after, act)))
            .collect(),
        gone: Mutex::default(),
        events: &events,
    };
    let mut play = Play::new(&f.base(), &events);
    for (issue, phases) in script {
        play = play.plays(*issue, phases);
    }
    let lock = RefCell::new(None);
    let lock_file = lock_path(&f.base());
    let env = Env {
        now: &|| clock.load(Ordering::SeqCst),
        sleep: &|d: Duration| {
            if lock.borrow().is_none() {
                *lock.borrow_mut() = fs::read_to_string(&lock_file).ok();
            }
            clock.fetch_add(d.as_secs(), Ordering::SeqCst);
        },
        stop_requested: stop,
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
        gh,
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

fn never() -> bool {
    false
}

/// The log rows of issue `issue`.
fn rows(f: &Fixture, issue: u32) -> Vec<String> {
    f.log_text()
        .lines()
        .filter(|l| l.contains(&format!("| #{issue} ")))
        .map(str::to_owned)
        .collect()
}

// --- boundary case 1: an adopted cloud child that ends `ok` ---

#[test]
fn an_adopted_cloud_child_that_ends_ready_is_finished_with_one_row_of_its_own_start() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 10, Act::Ready)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(run.out.contains("adopted"), "{}", run.out);
    assert!(run.starts().is_empty(), "{:?}\n{}", run.events, run.out);
    assert!(run.at("end 3 adopted") < run.at("finish 3"));
    let rec = read(&f.base(), 3);
    assert_eq!(rec.stage, Stage::Finished);
    assert!(rec.pr.is_some());
    let rows = rows(&f, 3);
    assert_eq!(rows.len(), 1, "{}", f.log_text());
    // Its own start, 09:00:00, not the new loop's.
    assert!(rows[0].contains("| cloud | 09:00:00 |"), "{}", rows[0]);
}

// --- boundary case 2: an adopted local child holds the local slot ---

#[test]
fn an_adopted_local_child_holds_the_local_slot_while_a_cloud_task_may_start() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Local, CHILD);
    let gh = github(&[5]);

    let run = run_loop(
        &f,
        &gh,
        &[(CHILD, 3, 600, Act::Ready)],
        &[(5, &[(3, Act::Ready)])],
        &never,
    );

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(run.starts(), ["start 5 run cloud"], "{}", run.out);
    assert!(run.at("start 5 run cloud") < run.at("end 3 adopted"));
    run.at("finish 3");
    run.at("finish 5");
}

// --- boundary case 3: adopted children count toward the 5-PR pause ---

#[test]
fn four_awaiting_prs_and_one_adopted_child_make_five_and_nothing_new_starts() {
    let f = fixture(Some(2), true);
    let mut gh = github(&[5]);
    for (issue, number) in [(50, 150), (51, 151), (52, 152), (53, 153)] {
        loop_pr(&f.base(), issue, number);
        gh = gh.with_pr(pr(number));
    }
    adoptable(&f.base(), 3, Location::Cloud, CHILD);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 600, Act::Ready)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(
        run.out
            .contains("paused: 4 PRs await you and 1 tasks run, 5 in all"),
        "{}",
        run.out
    );
    assert!(run.at("finish 3") < run.at("start 5 run local"));
}

// --- boundary case 4: an adopted child that ends with no outcome ---

#[test]
fn an_adopted_child_that_ends_with_no_outcome_is_resumed_once() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 10, Act::Crash)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(run.starts(), ["start 3 resume cloud"], "{}", run.out);
    assert!(run.at("end 3 adopted") < run.at("start 3 resume cloud"));
    run.at("finish 3");
    assert_eq!(
        read(&f.base(), 3).hooks[RESUMED_HOOK],
        serde_json::json!(true)
    );
    assert!(run.out.contains("resuming it once"), "{}", run.out);
}

#[test]
fn an_adopted_child_with_no_outcome_whose_resume_is_cut_off_again_is_parked_with_its_row() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[]);

    let run = run_loop(
        &f,
        &gh,
        &[(CHILD, 3, 10, Act::Crash)],
        &[(3, &[(0, Act::Crash)])],
        &never,
    );

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(run.starts(), ["start 3 resume cloud"], "{}", run.out);
    assert!(
        !run.events.contains(&"finish 3".to_owned()),
        "{:?}",
        run.events
    );
    let rows = rows(&f, 3);
    assert_eq!(rows.len(), 1, "{}", f.log_text());
    assert!(rows[0].contains("parked at working"), "{}", rows[0]);
}

#[test]
fn an_adopted_child_that_was_a_resume_already_taken_and_ends_with_no_outcome_is_parked() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let dir = record::task_dir(&f.base(), "issue-3");
    let mut rec = read(&f.base(), 3);
    rec.hooks = serde_json::json!({ RESUMED_HOOK: true });
    record::write(&dir, &rec).unwrap();
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 10, Act::Crash)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(run.starts().is_empty(), "{:?}\n{}", run.events, run.out);
    let rows = rows(&f, 3);
    assert_eq!(rows.len(), 1, "{}", f.log_text());
    assert!(rows[0].contains("parked at working"), "{}", rows[0]);
}

// --- boundary case 5: a live task run by hand is not adopted ---

#[test]
fn a_live_task_run_by_hand_is_not_adopted() {
    let f = fixture(Some(2), true);
    running(&f.base(), 3, Location::Cloud, Process::new(HAND, 0), None);
    let before =
        fs::read_to_string(record::task_dir(&f.base(), "issue-3").join("task.json")).unwrap();
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(run.events.is_empty(), "{:?}\n{}", run.events, run.out);
    assert!(!run.out.contains("adopted"), "{}", run.out);
    assert_eq!(
        fs::read_to_string(record::task_dir(&f.base(), "issue-3").join("task.json")).unwrap(),
        before
    );
}

// --- boundary case 6: a reused pid is not live ---

#[test]
fn a_child_whose_pid_was_reused_is_not_adopted_but_resumed_once() {
    let f = fixture(Some(2), true);
    // Recorded as started at 5; the pid is alive but started at 0: another process.
    running(
        &f.base(),
        3,
        Location::Cloud,
        Process::new(CHILD, 5),
        Some(DEAD_LOOP),
    );
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(!run.out.contains("adopted"), "{}", run.out);
    assert_eq!(run.starts(), ["start 3 resume cloud"], "{}", run.out);
    run.at("finish 3");
}

// --- boundary case 7: two adopted children end in the same tick ---

#[test]
fn two_adopted_children_ending_in_the_same_tick_are_finished_one_at_a_time() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    adoptable(&f.base(), 4, Location::Local, CHILD_2);
    let gh = github(&[]);

    let run = run_loop(
        &f,
        &gh,
        &[(CHILD, 3, 10, Act::Ready), (CHILD_2, 4, 10, Act::Ready)],
        &[],
        &never,
    );

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(
        run.events,
        ["end 3 adopted", "finish 3", "end 4 adopted", "finish 4"],
        "{}",
        run.out
    );
    assert_eq!(rows(&f, 3).len(), 1, "{}", f.log_text());
    assert_eq!(rows(&f, 4).len(), 1, "{}", f.log_text());
}

// --- boundary case 8: `--stop` while an adopted child runs ---

#[test]
fn a_stop_while_an_adopted_child_runs_starts_nothing_and_the_loop_exits_after_it_ends() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[5]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 60, Act::Ready)], &[], &|| true);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(run.starts().is_empty(), "{:?}\n{}", run.events, run.out);
    assert_eq!(run.events, ["end 3 adopted", "finish 3"], "{}", run.out);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
}

// --- boundary case 9: `cloud_slots` unset ---

#[test]
fn with_cloud_slots_unset_the_adopted_child_is_waited_for_before_the_next_task() {
    let f = fixture(None, true);
    adoptable(&f.base(), 3, Location::Local, CHILD);
    let gh = github(&[5]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 60, Act::Ready)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(
        run.events,
        [
            "end 3 adopted",
            "finish 3",
            "start 5 run local",
            "end 5 Ready",
            "finish 5"
        ],
        "{}",
        run.out
    );
    let rows = rows(&f, 3);
    assert_eq!(rows.len(), 1, "{}", f.log_text());
    assert!(rows[0].contains("| 09:00:00 |"), "{}", rows[0]);
}

#[test]
fn with_cloud_slots_unset_an_adopted_child_with_no_outcome_is_resumed_once() {
    let f = fixture(None, true);
    adoptable(&f.base(), 3, Location::Local, CHILD);
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 60, Act::Crash)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert_eq!(
        run.events,
        [
            "end 3 adopted",
            "start 3 resume local",
            "end 3 Ready",
            "finish 3"
        ],
        "{}",
        run.out
    );
    assert_eq!(
        read(&f.base(), 3).hooks[RESUMED_HOOK],
        serde_json::json!(true)
    );
}

// --- boundary case 10: a clean start adopts nothing ---

#[test]
fn a_clean_start_adopts_nothing() {
    let f = fixture(Some(2), false);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 10, Act::Ready)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    assert!(run.events.is_empty(), "{:?}\n{}", run.events, run.out);
    assert!(!run.out.contains("adopted"), "{}", run.out);
}

// --- `sbxm status` lists an adopted child as the loop's ---

/// A probe for which the pids in `.0` are gone.
struct Gone(&'static [u32]);

impl ProcessProbe for Gone {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (!self.0.contains(&pid)).then_some(0)
    }
}

fn lock_file(adopted: &[u32]) -> LockFile {
    LockFile {
        process: Process::new(1, 0),
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: adopted.to_vec(),
    }
}

#[test]
fn the_loop_names_the_children_it_adopted_in_its_lock() {
    let f = fixture(Some(2), true);
    adoptable(&f.base(), 3, Location::Cloud, CHILD);
    let gh = github(&[]);

    let run = run_loop(&f, &gh, &[(CHILD, 3, 10, Act::Ready)], &[], &never);

    assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
    let lock: LockFile = serde_json::from_str(run.lock.as_deref().expect("a lock")).unwrap();
    assert_eq!(lock.adopted, [3]);
}

#[test]
fn status_lists_an_adopted_child_as_the_running_loops() {
    let env = common::Env::new();
    let base = env.base_dir();
    adoptable(&base, 3, Location::Cloud, CHILD);
    let probe = Gone(&[DEAD_LOOP]);

    let work = looping::work_for(&base, &lock_file(&[3]), &probe).unwrap();

    let current = work.current.expect("the adopted child");
    assert_eq!(current.id, "issue-3");
    assert_eq!(current.slot, "cloud");
    // Not adopted: not the loop's.
    let work = looping::work_for(&base, &lock_file(&[]), &probe).unwrap();
    assert_eq!(work.current, None);
}

#[test]
fn status_does_not_list_a_task_of_an_adopted_issue_a_command_by_hand_took_since() {
    let env = common::Env::new();
    let base = env.base_dir();
    running(&base, 3, Location::Cloud, Process::new(HAND, 0), None);
    let probe = Gone(&[DEAD_LOOP]);

    let work = looping::work_for(&base, &lock_file(&[3]), &probe).unwrap();

    assert_eq!(work.current, None);
}
