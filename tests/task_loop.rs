//! Issue 320: `sbxm task loop` works through the `ready-for-worker` queue one task at a time
//! (decision 245 (b) to (h), the user's decisions of 2026-11-02). The task phases are scripted
//! (`Script`), writing the task records a real `task run`, `task resume` and `task finish` would;
//! the queue, the labels and the PRs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, PrInfo, PrState};
use sbxm::task::record::{
    self, Kind, NewTask, PrBranch, Process, ProcessProbe, Record, ReviewResult, Stage, Status,
};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;

/// What a scripted phase does.
#[derive(Clone)]
enum Act {
    /// Reaches `ready` with this many must-fix findings left.
    Ready(u32),
    /// Fails with this text, leaving the task's record at `working` when `true`.
    Fail(&'static str, bool),
    /// `sbx` couldn't run, before any record.
    ExecUnavailable,
    /// Reaches `ready`, and a command run by hand then holds the task's lock (issue 409):
    /// a live holder when `true`, a gone one otherwise.
    ReadyLocked(bool),
    /// `task resume` finds the task's lock held by a live command run by hand, which keeps it.
    Held,
}

/// A live process `Probe` reports for every pid: the holder of a lock planted by hand.
const HAND: u32 = 4242;

fn lock_file(base: &Path, issue: u32) -> PathBuf {
    record::task_dir(base, &format!("issue-{issue}")).join("lock.json")
}

/// Plants `issue-<n>`'s lock as a command run by hand holds it: alive under `Probe` when
/// `live`, else started at another time than `Probe` reports, so gone.
fn plant_lock(base: &Path, issue: u32, live: bool) -> String {
    let mut held = serde_json::to_value(Process::new(HAND, if live { 0 } else { 5 })).unwrap();
    held["token"] = "hand".into();
    let text = held.to_string();
    fs::write(lock_file(base, issue), &text).unwrap();
    text
}

/// Plays `task run`, `task resume`, `task finish` and `task file-findings` for the loop.
struct Script {
    base: PathBuf,
    acts: RefCell<HashMap<u32, VecDeque<Act>>>,
    calls: RefCell<Vec<String>>,
    /// The free-disk checks' answers, in order; `true` (room) once used up.
    room: RefCell<VecDeque<bool>>,
    clock: Cell<u64>,
    /// Seconds each `finish` takes on the clock.
    finish_takes: Cell<u64>,
    /// Issues whose `finish` fails, after taking its time.
    finish_fails: RefCell<Vec<u32>>,
    /// Issues whose task continues an open PR (decision 169), with that PR's number.
    continues: RefCell<HashMap<u32, u32>>,
    /// When this call is made, the command run by hand on this issue's task ends, freeing its
    /// lock, after leaving the task as [`Hand`] says.
    unlock_on: RefCell<Option<(String, u32, Hand)>>,
}

/// What the command run by hand leaves of the task when it ends (review M-1 of issue 409).
#[derive(Clone, Copy, PartialEq)]
enum Hand {
    /// The task as it found it.
    Unchanged,
    /// Finished into a PR, as `task finish` does.
    Finished,
    /// At `ready`, as `task resume` does.
    Ready,
    /// Gone, as `task rm` does.
    Removed,
    /// Stopped at `working`, as a failed `task resume --rounds` leaves it.
    Working,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            acts: RefCell::default(),
            calls: RefCell::default(),
            room: RefCell::default(),
            clock: Cell::new(START),
            finish_takes: Cell::new(0),
            finish_fails: RefCell::default(),
            continues: RefCell::default(),
            unlock_on: RefCell::default(),
        }
    }

    fn unlocking(self, call: &str, issue: u32) -> Self {
        self.hand_ends(call, issue, Hand::Unchanged)
    }

    fn hand_ends(self, call: &str, issue: u32, hand: Hand) -> Self {
        *self.unlock_on.borrow_mut() = Some((call.to_owned(), issue, hand));
        self
    }

    fn continuing(self, issue: u32, pr: u32) -> Self {
        self.continues.borrow_mut().insert(issue, pr);
        self
    }

    fn plays(self, issue: u32, acts: &[Act]) -> Self {
        self.acts
            .borrow_mut()
            .insert(issue, acts.iter().cloned().collect());
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn note(&self, call: String) {
        if let Some((_, locked, hand)) = self
            .unlock_on
            .borrow()
            .as_ref()
            .filter(|(when, ..)| *when == call)
        {
            let dir = record::task_dir(&self.base, &format!("issue-{locked}"));
            match hand {
                Hand::Unchanged => {}
                Hand::Finished => {
                    write_record(
                        &self.base,
                        *locked,
                        Stage::Finished,
                        None,
                        Some(500 + locked),
                    );
                }
                Hand::Ready => write_record(&self.base, *locked, Stage::Ready, Some(0), None),
                Hand::Removed => fs::remove_dir_all(&dir).unwrap(),
                Hand::Working => write_record(&self.base, *locked, Stage::Working, None, None),
            }
            let _ = fs::remove_file(lock_file(&self.base, *locked));
        }
        self.calls.borrow_mut().push(call);
    }

    fn act(&self, what: &str, issue: u32) -> Result<()> {
        self.note(format!("{what} {issue}"));
        self.clock.set(self.clock.get() + 300);
        let act = self
            .acts
            .borrow_mut()
            .get_mut(&issue)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Act::Ready(0));
        match act {
            Act::Ready(must_fix) => {
                write_record(&self.base, issue, Stage::Ready, Some(must_fix), None);
                if let Some(&pr) = self.continues.borrow().get(&issue) {
                    let mut rec = read_record(&self.base, issue);
                    rec.continues = Some(PrBranch {
                        pr,
                        branch: format!("fix-{pr}"),
                        base: "0".repeat(40),
                    });
                    record::write(&record::task_dir(&self.base, &rec.id), &rec).unwrap();
                }
                Ok(())
            }
            Act::Fail(text, leaves_record) => {
                if leaves_record {
                    write_record(&self.base, issue, Stage::Working, None, None);
                }
                Err(anyhow!("{text}"))
            }
            Act::ExecUnavailable => Err(anyhow::Error::new(sbxm::headless::ExecUnavailable(
                "sbx exec failed".into(),
            ))),
            Act::ReadyLocked(live) => {
                write_record(&self.base, issue, Stage::Ready, Some(0), None);
                plant_lock(&self.base, issue, live);
                Ok(())
            }
            Act::Held => {
                plant_lock(&self.base, issue, true);
                let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
                sbxm::task::lock::ProcessLock::acquire(&dir, &format!("task issue-{issue}"), &Probe)
                    .map(drop)
            }
        }
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        writeln!(out, "running #{issue}")?;
        self.act("run", issue)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.act("resume", issue)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.note(format!("finish {issue}"));
        self.clock.set(self.clock.get() + self.finish_takes.get());
        if self.finish_fails.borrow().contains(&issue) {
            return Err(anyhow!("push refused"));
        }
        let mut rec = read_record(&self.base, issue);
        rec.stage = Stage::Finished;
        // A continued PR keeps its number; a new one is 100 past the issue's.
        let pr = rec.continues.as_ref().map_or(100 + issue, |c| c.pr);
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
        record::write(&record::task_dir(&self.base, &rec.id), &rec).unwrap();
        Ok(())
    }

    fn file_findings(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.note(format!("file-findings {issue}"));
        Ok(())
    }

    fn room(&self) -> Result<()> {
        self.calls.borrow_mut().push("room".into());
        match self.room.borrow_mut().pop_front() {
            Some(false) => Err(anyhow!("only 1 GiB free")),
            _ => Ok(()),
        }
    }
}

fn read_record(base: &Path, issue: u32) -> Record {
    record::read(&record::task_dir(base, &format!("issue-{issue}")).join("task.json")).unwrap()
}

/// Writes `issue-<n>`'s record at `stage`, keeping one already there otherwise.
fn write_record(base: &Path, issue: u32, stage: Stage, must_fix: Option<u32>, pr: Option<u32>) {
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
    rec.status = if stage == Stage::Working {
        Status::Failed
    } else {
        Status::Ok
    };
    if let Some(must_fix) = must_fix {
        rec.review = Some(ReviewResult {
            round: 1,
            must_fix,
            ..ReviewResult::default()
        });
    }
    if let Some(pr) = pr {
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
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

    fn log(&self) -> PathBuf {
        self.repo_root.join("sdlc/batches/2026-11-02-loop.md")
    }

    fn log_text(&self) -> String {
        fs::read_to_string(self.log()).unwrap_or_default()
    }
}

fn labeled(number: u32, size: &str) -> Issue {
    open_issue(number, &[READY, size], "")
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

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: "abc".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

struct Run {
    result: Result<()>,
    out: String,
    warn: String,
    sleeps: Vec<Duration>,
}

fn run_loop(f: &Fixture, opts: Options, script: &Script, gh: &FakeGitHub) -> Run {
    run_loop_with(f, opts, script, gh, &Probe, &|| false)
}

fn run_loop_with(
    f: &Fixture,
    opts: Options,
    script: &Script,
    gh: &FakeGitHub,
    probe: &dyn ProcessProbe,
    stop: &dyn Fn() -> bool,
) -> Run {
    let sleeps = RefCell::new(Vec::new());
    let now = || script.clock.get();
    let sleep = |d: Duration| {
        sleeps.borrow_mut().push(d);
        script.clock.set(script.clock.get() + d.as_secs());
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
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        sleeps: sleeps.into_inner(),
    }
}

fn opts(f: &Fixture) -> Options {
    Options {
        repo_root: f.repo_root.clone(),
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
    }
}

fn with_cap(f: &Fixture, cap: u32) -> Options {
    Options {
        cap: Some(cap),
        ..opts(f)
    }
}

fn runs(script: &Script) -> Vec<String> {
    script
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("run ") || c.starts_with("resume "))
        .collect()
}

// --- the queue ---

#[test]
fn the_queue_runs_by_size_then_age_and_finishes_each_clean_task() {
    let f = fixture();
    let gh = github(vec![labeled(7, "size:M"), labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        script
            .calls()
            .into_iter()
            .filter(|c| c != "room")
            .collect::<Vec<_>>(),
        [
            "run 3",
            "finish 3",
            "file-findings 3",
            "run 7",
            "finish 7",
            "file-findings 7"
        ]
    );
    assert!(run.out.contains("queue is empty"), "{}", run.out);
}

#[test]
fn the_label_is_removed_when_a_task_starts() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    let calls = gh.calls();
    let removed = calls
        .iter()
        .position(|c| *c == GhCall::IssueLabels(REPO.into(), 3, Vec::new(), vec![READY.to_owned()]))
        .expect("the label is removed");
    assert!(
        !calls[removed..]
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(_, _, add, _) if !add.is_empty())),
        "the loop never adds a label: {calls:?}"
    );
}

#[test]
fn an_empty_queue_at_start_reports_and_runs_nothing() {
    let f = fixture();
    let gh = github(vec![open_issue(
        5,
        &["size:M"],
        "## Open points\n\n- Warn or refuse?\n",
    )]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert!(runs(&script).is_empty());
    assert!(run.out.contains("queue is empty"), "{}", run.out);
    assert!(run.out.contains("#5 Fix 5 (size:M)"), "{}", run.out);
    assert!(run.out.contains("Warn or refuse?"), "{}", run.out);
    assert!(!f.log().exists(), "no task, no log");
}

#[test]
fn a_queue_emptied_mid_loop_stops_with_the_report() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), open_issue(9, &["size:L"], "")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.out.contains("queue is empty"), "{}", run.out);
    assert!(run.out.contains("#9 Fix 9 (size:L)"), "{}", run.out);
}

#[test]
fn named_issues_run_in_the_users_order_and_the_list_ending_stops_the_loop() {
    let f = fixture();
    let gh = github(vec![
        labeled(3, "size:S"),
        labeled(7, "size:L"),
        open_issue(8, &[], ""),
    ]);
    let script = Script::new(&f.base());

    let run = run_loop(
        &f,
        Options {
            issues: vec![8, 7],
            ..opts(&f)
        },
        &script,
        &gh,
    );

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 8", "run 7"]);
    // #8 had no label: nothing to remove.
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(_, 8, _, _)))
    );
}

// --- --cap ---

#[test]
fn cap_0_runs_nothing_and_reads_nothing() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 0), &script, &gh);

    run.result.unwrap();
    assert!(script.calls().is_empty());
    assert!(gh.calls().is_empty());
    assert!(run.out.contains("--cap 0"), "{}", run.out);
}

#[test]
fn cap_1_runs_one_task() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.out.contains("--cap 1 reached"), "{}", run.out);
}

// --- outcomes ---

#[test]
fn a_work_stop_is_finished_into_a_draft_and_files_no_findings() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::Ready(2)]);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert!(script.calls().contains(&"finish 3".to_owned()));
    assert!(!script.calls().contains(&"file-findings 3".to_owned()));
    assert!(f.log_text().contains("draft PR #103"), "{}", f.log_text());
    assert!(f.log_text().contains("must-fix 2"), "{}", f.log_text());
}

#[test]
fn a_clean_task_is_logged_with_its_pr_and_times() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    let log = f.log_text();
    assert!(
        log.contains("| 1 | #3 Fix 3 | S | 10:00:00 | 10:05:00 (5m00s) |"),
        "{log}"
    );
    assert!(
        log.contains("| 1 (0) | PR #103, clean | none | - |"),
        "{log}"
    );
}

/// Review S-1: idle time runs from one task's ready to the next one's start (decision 249), so
/// the loop's own `task finish` and findings filing count as idle.
#[test]
fn the_idle_time_runs_from_ready_to_the_next_start() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());
    script.finish_takes.set(600);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    let log = f.log_text();
    let row = log.lines().find(|l| l.contains("| #4 ")).expect("#4's row");
    assert!(row.contains("| 10m00s |"), "{log}");
}

#[test]
fn a_failed_finish_keeps_its_ready_as_the_idle_baseline() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());
    script.finish_takes.set(600);
    script.finish_fails.borrow_mut().push(3);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    let log = f.log_text();
    let parked = log.lines().find(|l| l.contains("| #3 ")).expect("#3's row");
    assert!(parked.contains("finish failed"), "{log}");
    assert!(
        parked.contains("| 10:05:00 (5m00s) |"),
        "it reached ready: {log}"
    );
    let row = log.lines().find(|l| l.contains("| #4 ")).expect("#4's row");
    assert!(row.contains("| 10m00s |"), "{log}");
}

// --- the per-task lock (issue 409) ---

fn phases(script: &Script) -> Vec<String> {
    script.calls().into_iter().filter(|c| c != "room").collect()
}

fn loops_hook(base: &Path, issue: u32) -> Option<serde_json::Value> {
    read_record(base, issue)
        .hooks
        .get(task_loop::LOOP_HOOK)
        .cloned()
}

#[test]
fn a_free_lock_is_taken_for_the_claim_and_free_again_after_it() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert_eq!(
        loops_hook(&f.base(), 3),
        Some(serde_json::Value::Bool(true))
    );
    assert!(!lock_file(&f.base(), 3).exists());
}

#[test]
fn a_lock_held_at_the_claim_skips_the_finish_and_the_task_is_tried_after_the_next_one() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::ReadyLocked(true)])
        .unlocking("run 4", 3);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        [
            "run 3",
            "run 4",
            "finish 4",
            "file-findings 4",
            "finish 3",
            "file-findings 3"
        ]
    );
    let line = run
        .out
        .lines()
        .find(|l| l.contains("in use by another sbxm command"))
        .unwrap_or_else(|| panic!("{}", run.out));
    assert!(line.starts_with("#3: "), "{line}");
    assert!(line.contains(&format!("process {HAND}")), "{line}");
    assert!(line.contains("task finish"), "{line}");
    assert_eq!(
        run.out.matches("in use by another sbxm command").count(),
        1,
        "{}",
        run.out
    );
    assert_eq!(
        loops_hook(&f.base(), 3),
        Some(serde_json::Value::Bool(true))
    );
    assert!(!lock_file(&f.base(), 3).exists());
    let log = f.log_text();
    assert_eq!(log.matches("| #3 ").count(), 1, "{log}");
}

#[test]
fn a_lock_held_at_the_claim_writes_nothing_and_never_calls_finish() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::ReadyLocked(true)]);
    let task_json = record::task_dir(&f.base(), "issue-3").join("task.json");

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(phases(&script), ["run 3"]);
    assert_eq!(loops_hook(&f.base(), 3), None);
    assert_eq!(read_record(&f.base(), 3).stage, Stage::Ready);
    assert!(task_json.is_file());
    let lock: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(lock_file(&f.base(), 3)).unwrap()).unwrap();
    assert_eq!(lock["token"], "hand", "the hand command's lock stays");
    assert!(
        run.out.contains("sbxm task finish --issue 3"),
        "the loop's end names what is left: {}",
        run.out
    );
    assert!(!f.log_text().contains("| #3 "), "{}", f.log_text());
}

#[test]
fn a_stale_lock_at_the_claim_is_taken_over() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::ReadyLocked(false)]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(phases(&script), ["run 3", "finish 3", "file-findings 3"]);
    assert_eq!(
        loops_hook(&f.base(), 3),
        Some(serde_json::Value::Bool(true))
    );
    assert!(!lock_file(&f.base(), 3).exists());
    assert!(!run.out.contains("in use"), "{}", run.out);
}

#[test]
fn a_lock_held_at_the_resume_skips_it_and_the_task_is_tried_after_the_next_one() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(
            3,
            &[
                Act::Fail("sbx stopped answering", true),
                Act::Held,
                Act::Ready(0),
            ],
        )
        .unlocking("run 4", 3);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        [
            "run 3",
            "resume 3",
            "run 4",
            "finish 4",
            "file-findings 4",
            "resume 3",
            "finish 3",
            "file-findings 3"
        ]
    );
    let line = run
        .out
        .lines()
        .find(|l| l.contains("in use by another sbxm command"))
        .unwrap_or_else(|| panic!("{}", run.out));
    assert!(line.starts_with("#3: "), "{line}");
    assert!(line.contains("task resume"), "{line}");
    assert!(!run.out.contains("parked"), "{}", run.out);
}

#[test]
fn a_held_resume_is_not_a_failure_so_the_retry_still_counts_the_first_one() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(
            3,
            &[
                Act::Fail("sbx stopped answering", true),
                Act::Held,
                Act::Fail("sbx stopped answering", true),
            ],
        )
        .unlocking("run 4", 3);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        runs(&script),
        ["run 3", "resume 3", "run 4", "resume 3"],
        "the retry's failure is the task's second: parked, not resumed again"
    );
    assert!(run.out.contains("#3: parked"), "{}", run.out);
}

#[test]
fn a_task_held_with_nothing_else_to_run_ends_the_loop_naming_its_resume() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script =
        Script::new(&f.base()).plays(3, &[Act::Fail("sbx stopped answering", true), Act::Held]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3", "resume 3"]);
    assert!(
        run.out.contains("sbxm task resume --issue 3"),
        "{}",
        run.out
    );
}

/// Review M-1: a task the command run by hand finished while the loop waited is that command's:
/// no claim, no second finish, no row and not parked.
#[test]
fn a_held_finish_whose_holder_finished_the_task_is_left_to_it() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::ReadyLocked(true)])
        .hand_ends("run 4", 3, Hand::Finished);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        ["run 3", "run 4", "finish 4", "file-findings 4"]
    );
    assert_eq!(
        loops_hook(&f.base(), 3),
        None,
        "a PR opened by hand is not the loop's"
    );
    assert!(!run.out.contains("parked"), "{}", run.out);
    let line = run
        .out
        .lines()
        .find(|l| l.starts_with("#3: ") && l.contains("finished by a command run by hand"))
        .unwrap_or_else(|| panic!("{}", run.out));
    assert!(line.contains("pull/503"), "{line}");
    assert!(!f.log_text().contains("| #3 "), "{}", f.log_text());
    assert!(
        !run.out.contains("sbxm task finish --issue 3"),
        "{}",
        run.out
    );
}

#[test]
fn a_held_resume_whose_holder_finished_the_task_is_left_to_it() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::Fail("sbx stopped answering", true), Act::Held])
        .hand_ends("run 4", 3, Hand::Finished);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        ["run 3", "resume 3", "run 4", "finish 4", "file-findings 4"]
    );
    assert_eq!(loops_hook(&f.base(), 3), None);
    assert!(!run.out.contains("parked"), "{}", run.out);
    assert!(
        run.out.contains("finished by a command run by hand"),
        "{}",
        run.out
    );
}

#[test]
fn a_held_resume_whose_holder_got_the_task_ready_goes_on_to_its_finish() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::Fail("sbx stopped answering", true), Act::Held])
        .hand_ends("run 4", 3, Hand::Ready);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        [
            "run 3",
            "resume 3",
            "run 4",
            "finish 4",
            "file-findings 4",
            "finish 3",
            "file-findings 3"
        ],
        "a ready task is not resumed again"
    );
    assert_eq!(
        loops_hook(&f.base(), 3),
        Some(serde_json::Value::Bool(true))
    );
}

#[test]
fn a_held_finish_whose_holder_left_the_task_short_of_ready_resumes_it() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::ReadyLocked(true), Act::Ready(0)])
        .hand_ends("run 4", 3, Hand::Working);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert_eq!(
        phases(&script),
        [
            "run 3",
            "run 4",
            "finish 4",
            "file-findings 4",
            "resume 3",
            "finish 3",
            "file-findings 3"
        ]
    );
}

#[test]
fn a_held_task_its_holder_removed_is_left_to_it() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::ReadyLocked(true)])
        .hand_ends("run 4", 3, Hand::Removed);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        phases(&script),
        ["run 3", "run 4", "finish 4", "file-findings 4"],
        "neither finished nor run again"
    );
    assert!(!run.out.contains("parked"), "{}", run.out);
    assert!(
        run.out
            .contains("#3: its task was removed by a command run by hand"),
        "{}",
        run.out
    );
    assert!(!f.log_text().contains("| #3 "), "{}", f.log_text());
}

#[test]
fn a_held_task_does_not_count_toward_the_cap() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base())
        .plays(3, &[Act::ReadyLocked(true)])
        .unlocking("run 4", 3);

    run_loop(&f, with_cap(&f, 1), &script, &gh).result.unwrap();

    assert_eq!(
        phases(&script),
        ["run 3", "run 4", "finish 4", "file-findings 4"]
    );
}

// --- environment stops ---

#[test]
fn one_environment_failure_is_resumed_once() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(
        3,
        &[Act::Fail("sbx stopped answering", true), Act::Ready(0)],
    );

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3", "resume 3"]);
    assert!(script.calls().contains(&"finish 3".to_owned()));
    assert!(
        f.log_text().contains("sbx stopped answering"),
        "{}",
        f.log_text()
    );
}

#[test]
fn an_environment_failure_before_any_record_is_run_again_once() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::ExecUnavailable, Act::Ready(0)]);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert_eq!(runs(&script), ["run 3", "run 3"]);
}

#[test]
fn two_failures_in_a_row_park_the_task_and_the_loop_goes_on() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:M")]);
    let script = Script::new(&f.base()).plays(
        3,
        &[
            Act::Fail("gate could not run", true),
            Act::Fail("gate could not run", true),
        ],
    );

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3", "resume 3", "run 4"]);
    assert!(!script.calls().contains(&"finish 3".to_owned()));
    assert!(run.out.contains("parked"), "{}", run.out);
    assert!(
        f.log_text().contains("parked at working"),
        "{}",
        f.log_text()
    );
}

#[test]
fn two_parked_tasks_in_a_row_stop_the_loop() {
    let f = fixture();
    let gh = github(vec![
        labeled(3, "size:S"),
        labeled(4, "size:S"),
        labeled(5, "size:S"),
    ]);
    let fail = [Act::Fail("killed", true), Act::Fail("killed", true)];
    let script = Script::new(&f.base()).plays(3, &fail).plays(4, &fail);

    let run = run_loop(&f, opts(&f), &script, &gh);

    let err = format!("{:#}", run.result.unwrap_err());
    assert!(err.contains("two tasks parked in a row"), "{err}");
    assert_eq!(runs(&script), ["run 3", "resume 3", "run 4", "resume 4"]);
    assert_eq!(f.log_text().matches("parked at").count(), 2);
}

#[test]
fn a_task_reaching_ready_between_parked_ones_resets_the_count() {
    let f = fixture();
    let gh = github(vec![
        labeled(3, "size:S"),
        labeled(4, "size:S"),
        labeled(5, "size:S"),
    ]);
    let fail = [Act::Fail("killed", true), Act::Fail("killed", true)];
    let script = Script::new(&f.base()).plays(3, &fail).plays(5, &fail);

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert_eq!(
        runs(&script),
        ["run 3", "resume 3", "run 4", "run 5", "resume 5"]
    );
}

#[test]
fn the_loop_pauses_while_the_disk_is_low_and_goes_on_once_it_has_room() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());
    script.room.borrow_mut().extend([false, false]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(run.sleeps.len(), 2, "{:?}", run.sleeps);
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.out.contains("only 1 GiB free"), "{}", run.out);
    assert!(f.log_text().contains("disk, 10m00s"), "{}", f.log_text());
}

/// The user's Ctrl-C answer (2026-11-02): the disk pause asks for a stop after each check.
#[test]
fn a_stop_asked_during_the_disk_pause_ends_the_loop_before_any_task() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());
    script.room.borrow_mut().extend([false, false, false]);
    let asked = || script.clock.get() > START;

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Probe, &asked);

    run.result.unwrap();
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL]);
    assert!(runs(&script).is_empty(), "{:?}", runs(&script));
    assert!(run.out.contains("stopped on request"), "{}", run.out);
}

// --- Ctrl-C ---

#[test]
fn the_first_ctrl_c_asks_for_a_stop_and_the_second_stops_now() {
    let flag = task_loop::StopFlag::default();
    assert!(!flag.requested());

    assert_eq!(flag.press(), task_loop::Press::First);
    assert!(flag.requested());
    assert_eq!(flag.press(), task_loop::Press::Again);
    assert_eq!(flag.press(), task_loop::Press::Again);
}

// --- the usage limit ---

#[test]
fn a_usage_limit_is_waited_out_then_resumed() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(
        3,
        &[
            Act::Fail("Claude AI usage limit reached", true),
            Act::Ready(0),
        ],
    );

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    // The 30m wait sleeps in `PAUSE_POLL` steps, so a stop request is seen within one.
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL; 6]);
    assert_eq!(runs(&script), ["run 3", "resume 3"]);
    assert!(run.out.contains("usage limit"), "{}", run.out);
    assert!(
        f.log_text().contains("usage limit, 30m00s"),
        "{}",
        f.log_text()
    );
}

#[test]
fn usage_waits_reaching_the_total_stop_the_loop() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let limit = Act::Fail("You've hit your usage limit", true);
    let script = Script::new(&f.base()).plays(3, &[limit.clone(), limit.clone(), limit]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    let err = format!("{:#}", run.result.unwrap_err());
    assert!(err.contains("usage limit"), "{err}");
    assert!(err.contains("1h00m00s"), "{err}");
    // 30m twice reaches the 1h total: the third limit stops the loop.
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL; 12]);
    assert_eq!(runs(&script), ["run 3", "resume 3", "resume 3"]);
}

#[test]
fn a_usage_wait_equal_to_its_total_waits_once() {
    let f = fixture();
    fs::write(
        f.repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\nusage_wait = \
         \"1h\"\nusage_wait_total = \"1h\"\n",
    )
    .unwrap();
    let gh = github(vec![labeled(3, "size:S")]);
    let limit = Act::Fail("usage limit reached", true);
    let script = Script::new(&f.base()).plays(3, &[limit.clone(), limit]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    assert!(run.result.is_err());
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL; 12]);
}

/// `usage_wait` set to `wait` in the repo's `sbxm-task.toml`, with a 5h total.
fn with_usage_wait(f: &Fixture, wait: &str) {
    fs::write(
        f.repo_root.join("sbxm-task.toml"),
        format!(
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\nusage_wait = \
             \"{wait}\"\nusage_wait_total = \"5h\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_usage_wait_shorter_than_the_poll_is_one_sleep_of_its_length() {
    let f = fixture();
    with_usage_wait(&f, "1m");
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::Fail("usage limit reached", true)]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(run.sleeps, [Duration::from_secs(60)]);
    assert_eq!(runs(&script), ["run 3", "resume 3"]);
}

#[test]
fn a_usage_wait_equal_to_the_poll_is_one_sleep() {
    let f = fixture();
    with_usage_wait(&f, "5m");
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::Fail("usage limit reached", true)]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL]);
}

/// The user's Ctrl-C answer (2026-11-02): a stop asked during a usage wait ends the task after
/// the step in which it was asked, before its resume, naming the command that continues it.
#[test]
fn a_stop_asked_during_a_usage_wait_ends_it_after_that_step() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::Fail("usage limit reached", true)]);
    // The scripted run takes 300 s on the clock; the stop is asked during the third step.
    let asked = || script.clock.get() >= START + 300 + 3 * task_loop::PAUSE_POLL.as_secs();

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Probe, &asked);

    run.result.unwrap();
    assert_eq!(run.sleeps, [task_loop::PAUSE_POLL; 3]);
    assert_eq!(runs(&script), ["run 3"]);
    assert!(
        run.out.contains("sbxm task resume --issue 3"),
        "{}",
        run.out
    );
    assert!(run.out.contains("stopped on request"), "{}", run.out);
    assert!(
        f.log_text().contains("usage limit, 15m00s"),
        "{}",
        f.log_text()
    );
}

// --- the PR cap ---

/// `count` finished tasks whose PRs are open, numbered from 50; `armed` of them auto-merge.
/// Marks task `id`'s record as one the loop finished, for `repo`.
fn loops_own(base: &Path, id: &str, repo: &str) {
    let dir = record::task_dir(base, id);
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.repo = repo.into();
    rec.hooks[task_loop::LOOP_HOOK] = serde_json::Value::Bool(true);
    record::write(&dir, &rec).unwrap();
}

/// `count` finished issue tasks the loop opened open PRs for (`#1050` on), `armed` of them with
/// auto-merge armed.
fn awaiting(f: &Fixture, gh: FakeGitHub, count: u32, armed: u32) -> FakeGitHub {
    let mut gh = gh;
    for i in 0..count {
        let issue = 50 + i;
        write_record(
            &f.base(),
            issue,
            Stage::Finished,
            Some(0),
            Some(issue + 1000),
        );
        loops_own(&f.base(), &format!("issue-{issue}"), REPO);
        gh = gh.with_pr(pr(issue + 1000, PrState::Open));
        if i < armed {
            gh = gh.with_auto_merge(issue + 1000);
        }
    }
    gh
}

#[test]
fn four_prs_awaiting_the_user_let_the_loop_run() {
    let f = fixture();
    let gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 4, 0);
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.sleeps.is_empty());
}

#[test]
fn five_prs_awaiting_the_user_pause_the_loop() {
    let f = fixture();
    let gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 5, 0);
    let script = Script::new(&f.base());
    // The user merges one while the loop waits: its task is gone (`task rm --merged`).
    let checks = Cell::new(0);
    let base = f.base();
    let stop = || {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            fs::remove_dir_all(record::task_dir(&base, "issue-50")).unwrap();
        }
        false
    };

    let run = run_loop_with(&f, with_cap(&f, 1), &script, &gh, &Probe, &stop);

    run.result.unwrap();
    assert!(!run.sleeps.is_empty());
    assert!(run.out.contains("5 PRs await you"), "{}", run.out);
    assert_eq!(runs(&script), ["run 3"]);
}

#[test]
fn five_prs_with_one_armed_let_the_loop_run() {
    let f = fixture();
    let gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 5, 1);
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.sleeps.is_empty());
}

/// Review M-3: the cap counts the PRs a loop opened, not every task's.
#[test]
fn five_hand_finished_prs_do_not_pause_a_loop_that_opened_none() {
    let f = fixture();
    let mut gh = github(vec![labeled(3, "size:S")]);
    for issue in 50..55 {
        write_record(
            &f.base(),
            issue,
            Stage::Finished,
            Some(0),
            Some(issue + 1000),
        );
        gh = gh.with_pr(pr(issue + 1000, PrState::Open));
    }
    let script = Script::new(&f.base());

    // A pause would wait for ever: asked to stop after the loop's first look at the PRs.
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let run = run_loop_with(&f, with_cap(&f, 1), &script, &gh, &Probe, &stop);

    run.result.unwrap();
    assert!(run.sleeps.is_empty(), "{}", run.out);
    assert_eq!(runs(&script), ["run 3"]);
}

#[test]
fn another_repos_loop_prs_do_not_count() {
    let f = fixture();
    let mut gh = github(vec![labeled(3, "size:S")]);
    for issue in 50..55 {
        write_record(
            &f.base(),
            issue,
            Stage::Finished,
            Some(0),
            Some(issue + 1000),
        );
        loops_own(&f.base(), &format!("issue-{issue}"), "o/other");
        gh = gh.with_pr(pr(issue + 1000, PrState::Open));
    }
    let script = Script::new(&f.base());

    // A pause would wait for ever: asked to stop after the loop's first look at the PRs.
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let run = run_loop_with(&f, with_cap(&f, 1), &script, &gh, &Probe, &stop);

    run.result.unwrap();
    assert!(run.sleeps.is_empty(), "{}", run.out);
    assert_eq!(runs(&script), ["run 3"]);
}

#[test]
fn five_prs_a_loop_opened_pause_the_next_loop() {
    let f = fixture();
    let mut issues: Vec<Issue> = (1..=5).map(|n| labeled(n, "size:S")).collect();
    issues.push(labeled(6, "size:M"));
    let mut gh = github(issues);
    for issue in 1..=5 {
        gh = gh.with_pr(pr(100 + issue, PrState::Open));
    }
    run_loop(&f, with_cap(&f, 5), &Script::new(&f.base()), &gh)
        .result
        .unwrap();

    let again = Script::new(&f.base());
    // Asked to stop after its first look at the PRs.
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let run = run_loop_with(&f, opts(&f), &again, &gh, &Probe, &stop);

    assert!(run.out.contains("5 PRs await you"), "{}", run.out);
    assert!(runs(&again).is_empty());
}

#[test]
fn five_tasks_continuing_prs_opened_by_hand_do_not_pause_the_next_loop() {
    let f = fixture();
    let mut issues: Vec<Issue> = (1..=5).map(|n| labeled(n, "size:S")).collect();
    issues.push(labeled(6, "size:M"));
    let mut gh = github(issues);
    let mut script = Script::new(&f.base());
    for issue in 1..=5 {
        // Each continues an open PR opened by hand (decision 169).
        gh = gh.with_pr(pr(100 + issue, PrState::Open));
        script = script.continuing(issue, 100 + issue);
    }
    run_loop(&f, with_cap(&f, 5), &script, &gh).result.unwrap();

    let again = Script::new(&f.base());
    // A pause would wait for ever: asked to stop after the loop's first look at the PRs.
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let run = run_loop_with(&f, with_cap(&f, 1), &again, &gh, &Probe, &stop);

    run.result.unwrap();
    assert!(!run.out.contains("await you"), "{}", run.out);
    assert_eq!(runs(&again), ["run 6"]);
}

#[test]
fn a_merged_pr_is_not_counted() {
    let f = fixture();
    let mut gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 4, 0);
    write_record(&f.base(), 60, Stage::Finished, Some(0), Some(1060));
    loops_own(&f.base(), "issue-60", REPO);
    gh = gh.with_pr(pr(1060, PrState::Merged));
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert!(run.sleeps.is_empty());
}

#[test]
fn a_task_naming_a_pr_that_no_longer_exists_is_left_out_with_a_warning() {
    let f = fixture();
    let gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 4, 0);
    // PR #1099 isn't scripted: `gh` can't find it.
    write_record(&f.base(), 99, Stage::Finished, Some(0), Some(1099));
    loops_own(&f.base(), "issue-99", REPO);
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert!(run.sleeps.is_empty());
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.warn.contains("issue-99"), "{}", run.warn);
    assert!(run.warn.contains("#1099"), "{}", run.warn);
    assert_eq!(run.warn.matches("#1099").count(), 1, "once: {}", run.warn);
}

// --- what is skipped or refused ---

#[test]
fn an_issue_depending_on_a_merged_pr_is_picked() {
    let f = fixture();
    let gh = github(vec![open_issue(
        3,
        &[READY, "size:S"],
        "**Depends on:** #341 and PR #377\n",
    )])
    .with_pr(pr(377, PrState::Merged));
    // #341 is neither open nor a PR: a closed issue.
    let script = Script::new(&f.base());

    run_loop(&f, opts(&f), &script, &gh).result.unwrap();

    assert_eq!(runs(&script), ["run 3"]);
}

#[test]
fn an_issue_depending_on_an_open_pr_or_issue_is_skipped_and_reported() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &[READY, "size:S"], "**Depends on:** PR #377\n"),
        open_issue(4, &[READY, "size:S"], "**Depends on:** #9\n"),
        open_issue(9, &["size:L"], ""),
        labeled(5, "size:M"),
    ])
    .with_pr(pr(377, PrState::Open));
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 5"]);
    assert!(run.out.contains("#3 Fix 3: "), "{}", run.out);
    assert!(run.out.contains("#377"), "{}", run.out);
    assert!(run.out.contains("#4 Fix 4: "), "{}", run.out);
}

#[test]
fn the_first_task_refused_before_any_write_is_reported_and_the_loop_goes_on() {
    let f = fixture();
    // #3's body names a merged PR: `task run` would refuse it before any write.
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(sbxm::github::IssueText {
            number: 3,
            title: "Fix 3".into(),
            state: "OPEN".into(),
            text: "title:\tFix 3\nstate:\tOPEN\n--\nPR: #200\n\nfix it\n".into(),
        })
        .with_issue_text(issue_text(4))
        .with_open_issues(vec![
            open_issue(3, &[READY, "size:S"], "PR: #200\n\nfix it"),
            labeled(4, "size:M"),
        ])
        .with_pr(pr(200, PrState::Merged));
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 4"]);
    assert!(run.out.contains("#3"), "{}", run.out);
    assert!(run.out.contains("refused"), "{}", run.out);
    // Its label stays: no task started.
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(_, 3, _, _)))
    );
    assert!(!record::task_dir(&f.base(), "issue-3").exists());
}

// --- reruns, links, the log and the lock ---

#[test]
fn a_rerun_after_a_kill_runs_no_task_twice_and_logs_none_twice() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());
    run_loop(&f, with_cap(&f, 1), &script, &gh).result.unwrap();

    // Killed after `task finish` of #3 and before #4 started; then run again, naming #3 too.
    let again = Script::new(&f.base());
    let run = run_loop(
        &f,
        Options {
            issues: vec![3, 4],
            ..opts(&f)
        },
        &again,
        &gh,
    );

    run.result.unwrap();
    assert_eq!(runs(&again), ["run 4"]);
    assert_eq!(f.log_text().matches("| #3 ").count(), 1, "{}", f.log_text());
    assert_eq!(f.log_text().matches("| #4 ").count(), 1, "{}", f.log_text());
}

#[test]
fn a_linked_tasks_folder_is_refused_before_the_first_task() {
    let f = fixture();
    let elsewhere = f.env.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let tasks = record::tasks_root(&f.base());
    fs::create_dir_all(tasks.parent().unwrap()).unwrap();
    common::dir_link(&tasks, &elsewhere);
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    let err = format!("{:#}", run.result.unwrap_err());
    assert!(err.contains("symlink or junction"), "{err}");
    assert!(script.calls().is_empty());
    assert!(gh.calls().is_empty());
}

#[test]
fn a_log_that_cannot_be_written_is_a_warning_and_the_loop_goes_on() {
    let f = fixture();
    // A folder where the log file should be.
    fs::create_dir_all(f.log()).unwrap();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3", "run 4"]);
    assert!(run.warn.contains("cannot"), "{}", run.warn);
    assert!(run.warn.contains("loop.md"), "{}", run.warn);
}

struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, pid: u32) -> Option<u64> {
        (pid == 4242).then_some(7).or(Some(0))
    }
}

#[test]
fn a_second_loop_is_refused_by_the_lock() {
    let f = fixture();
    let lock = f.base().join(".sbxm").join("task-loop.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(
        &lock,
        serde_json::to_string(&Process::new(4242, 7)).unwrap(),
    )
    .unwrap();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Alive, &|| false);

    let err = format!("{:#}", run.result.unwrap_err());
    assert!(err.contains("already running"), "{err}");
    assert!(err.contains("4242"), "{err}");
    assert!(script.calls().is_empty());
    assert!(lock.exists(), "the other loop's lock stays");
}

#[test]
fn a_lock_left_by_a_dead_loop_is_taken_over_and_removed_at_the_end() {
    let f = fixture();
    let lock = f.base().join(".sbxm").join("task-loop.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(
        &lock,
        serde_json::to_string(&Process::new(4242, 9)).unwrap(),
    )
    .unwrap();
    let gh = github(vec![labeled(3, "size:S")]);
    let script = Script::new(&f.base());

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Alive, &|| false);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(!lock.exists());
}

#[test]
fn a_stop_request_ends_the_loop_after_the_current_task() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base());
    let asked = || script.calls().contains(&"finish 3".to_owned());

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Probe, &asked);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3"]);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
}

// --- the command line ---

#[test]
fn task_loop_is_a_command_with_cap_and_issue() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "loop", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{help}");
    assert!(help.contains("--cap"), "{help}");
    assert!(help.contains("--issue"), "{help}");
    assert!(help.contains("ready-for-worker"), "{help}");
}

#[test]
fn task_loop_with_cap_0_runs_nothing_from_the_command_line() {
    let f = fixture();
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(&f.repo_root)
        .env("SBXM_CONFIG_DIR", f.env.config_dir())
        .args(["task", "loop", "--cap", "0"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--cap 0: no task runs"));
}

// --- self-review fixes ---

#[test]
fn a_run_refused_before_its_record_keeps_the_label() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S"), labeled(4, "size:S")]);
    let script = Script::new(&f.base()).plays(3, &[Act::Fail("profile nope not found", false)]);

    let run = run_loop(&f, opts(&f), &script, &gh);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 3", "run 4"]);
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(_, 3, _, _))),
        "{:?}",
        gh.calls()
    );
    assert!(run.out.contains("profile nope not found"), "{}", run.out);
}

#[test]
fn only_issue_tasks_prs_count_and_each_once() {
    let f = fixture();
    let mut gh = awaiting(&f, github(vec![labeled(3, "size:S")]), 4, 0);
    // A second task continuing PR #1050 (decision 169) and a PR task naming PR #1070.
    write_record(&f.base(), 70, Stage::Finished, Some(0), Some(1050));
    loops_own(&f.base(), "issue-70", REPO);
    let dir = record::task_dir(&f.base(), "pr-1070");
    let mut rec = read_record(&f.base(), 70);
    rec.id = "pr-1070".into();
    rec.kind = Kind::Pr;
    rec.number = 1070;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/1070"));
    record::write(&dir, &rec).unwrap();
    gh = gh.with_pr(pr(1070, PrState::Open));
    let script = Script::new(&f.base());

    let run = run_loop(&f, with_cap(&f, 1), &script, &gh);

    run.result.unwrap();
    assert!(run.sleeps.is_empty(), "{}", run.out);
}

#[test]
fn an_issue_whose_dependency_closes_mid_loop_is_picked_then() {
    let f = fixture();
    let blocked = open_issue(4, &[READY, "size:S"], "**Depends on:** #9\n");
    let gh = github(vec![
        blocked.clone(),
        open_issue(9, &["size:L"], ""),
        labeled(5, "size:M"),
    ]);
    let script = Script::new(&f.base());
    // #9 is closed while #5 runs.
    let stop = || {
        if script.calls().contains(&"finish 5".to_owned()) {
            gh.set_open_issues(vec![blocked.clone()]);
        }
        false
    };

    let run = run_loop_with(&f, opts(&f), &script, &gh, &Probe, &stop);

    run.result.unwrap();
    assert_eq!(runs(&script), ["run 5", "run 4"]);
}

#[test]
fn the_issues_own_words_about_a_usage_limit_are_not_a_usage_limit() {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    struct Wordy(Script);
    impl Steps for Wordy {
        fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
            writeln!(out, "the issue: wait out the usage limit")?;
            self.0.run(issue, out, warn)
        }
        fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
            self.0.resume(issue, out, warn)
        }
        fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
            self.0.finish(issue, out, warn)
        }
        fn file_findings(
            &self,
            issue: u32,
            out: &mut dyn Write,
            warn: &mut dyn Write,
        ) -> Result<()> {
            self.0.file_findings(issue, out, warn)
        }
        fn room(&self) -> Result<()> {
            self.0.room()
        }
    }
    let script =
        Wordy(Script::new(&f.base()).plays(3, &[Act::Fail("1 task failed", true), Act::Ready(0)]));

    let sleeps = RefCell::new(Vec::new());
    let now = || script.0.clock.get();
    let sleep = |d: Duration| sleeps.borrow_mut().push(d);
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &|| false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run(
        &f.env.config_dir(),
        &opts(&f),
        &script,
        &gh,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    )
    .unwrap();

    assert!(sleeps.borrow().is_empty(), "no usage wait");
    assert_eq!(runs(&script.0), ["run 3", "resume 3"]);
}

// --- the real steps ---

#[test]
fn the_real_file_findings_step_files_only_should_fix_findings() {
    let f = fixture();
    write_record(&f.base(), 12, Stage::Finished, Some(0), Some(112));
    let review = include_str!("fixtures/review-findings/review-small.md")
        .replace("\r\n", "\n")
        .replace("## Must fix", "## Should fix");
    fs::write(
        record::task_dir(&f.base(), "issue-12").join("review.md"),
        review,
    )
    .unwrap();
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix", "should-fix", "question", "size:S", "size:M"])
        .with_next_issue_number(40);
    let backend = sbxm::backend::FakeBackend::default();
    let host = sbxm::task::gates::FakeHostRunner::default();
    let steps = task_loop::TaskSteps {
        config_dir: &f.env.config_dir(),
        repo_root: f.repo_root.clone(),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
    };

    steps
        .file_findings(12, &mut Vec::new(), &mut Vec::new())
        .unwrap();

    let created: Vec<String> = gh
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueCreate(_, request) => Some(request.title),
            _ => None,
        })
        .collect();
    assert_eq!(created.len(), 2, "{created:?}");
    assert!(
        created.iter().all(|t| !t.contains("Keep the thing")),
        "{created:?}"
    );
}

#[test]
fn the_real_file_findings_step_files_nothing_for_a_review_without_should_fix() {
    let f = fixture();
    write_record(&f.base(), 12, Stage::Finished, Some(0), Some(112));
    fs::write(
        record::task_dir(&f.base(), "issue-12").join("review.md"),
        "Must-fix findings: 0\n\nRisk: low\n\n## Summary\n\nFine.\n",
    )
    .unwrap();
    let gh = FakeGitHub::default().with_user("me");
    let backend = sbxm::backend::FakeBackend::default();
    let host = sbxm::task::gates::FakeHostRunner::default();
    let steps = task_loop::TaskSteps {
        config_dir: &f.env.config_dir(),
        repo_root: f.repo_root.clone(),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
    };

    steps
        .file_findings(12, &mut Vec::new(), &mut Vec::new())
        .unwrap();

    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
}

/// Issue 320 review M-1: the loop runs `task run` in its own process, so a failed phase leaves
/// the task owned by the loop itself; the real resume must take it over once that phase has
/// returned, and still refuse a task another live process owns.
fn resume_owned_by(f: &Fixture, owner: Process) -> (String, Record) {
    resume_locked(f, owner, None).0
}

/// [`resume_owned_by`] with `issue-3`'s lock planted first by a command run by hand, live or
/// gone (issue 409); also returns the error itself.
fn resume_locked(
    f: &Fixture,
    owner: Process,
    lock: Option<bool>,
) -> ((String, Record), Option<anyhow::Error>) {
    write_record(&f.base(), 3, Stage::Working, None, None);
    let dir = record::task_dir(&f.base(), "issue-3");
    let mut rec = read_record(&f.base(), 3);
    rec.process = Some(owner);
    record::write(&dir, &rec).unwrap();
    if let Some(live) = lock {
        plant_lock(&f.base(), 3, live);
    }
    let gh = FakeGitHub::default().with_user("me");
    let backend = sbxm::backend::FakeBackend::default();
    let host = sbxm::task::gates::FakeHostRunner::default();
    let steps = task_loop::TaskSteps {
        config_dir: &f.env.config_dir(),
        repo_root: f.repo_root.clone(),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
    };
    let result = steps.resume(3, &mut Vec::new(), &mut Vec::new());
    let error = match &result {
        Ok(()) => String::new(),
        Err(e) => format!("{e:#}"),
    };
    ((error, read_record(&f.base(), 3)), result.err())
}

#[test]
fn the_real_release_skips_a_task_whose_lock_a_live_command_holds() {
    let f = fixture();
    let me = Process::current(&Probe);
    let task_json = record::task_dir(&f.base(), "issue-3").join("task.json");

    let ((error, rec), err) = resume_locked(&f, me.clone(), Some(true));

    let err = err.expect("refused");
    assert!(
        err.is::<sbxm::task::lock::InUse>(),
        "the lock's refusal, not task resume's: {error}"
    );
    assert!(error.contains(&format!("process {HAND}")), "{error}");
    assert_eq!(rec.process, Some(me), "nothing written");
    assert!(task_json.is_file());
    let lock: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(lock_file(&f.base(), 3)).unwrap()).unwrap();
    assert_eq!(lock["token"], "hand", "the hand command's lock stays");
    let run_log = record::task_dir(&f.base(), "issue-3").join("run.log");
    assert!(
        !run_log.exists() || !fs::read_to_string(&run_log).unwrap().contains("resume"),
        "task resume never ran"
    );
}

#[test]
fn the_real_release_takes_over_a_stale_lock_and_frees_it() {
    let f = fixture();

    let ((error, _), _) = resume_locked(&f, Process::current(&Probe), Some(false));

    assert!(!error.contains("in use"), "{error}");
    assert!(!error.contains("still running"), "{error}");
    assert!(!lock_file(&f.base(), 3).exists());
}

#[test]
fn the_real_release_leaves_the_lock_free_once_the_loops_own_command_returned() {
    let f = fixture();

    let ((error, _), _) = resume_locked(&f, Process::current(&Probe), None);

    assert!(!error.contains("in use"), "{error}");
    assert!(!lock_file(&f.base(), 3).exists());
}

#[test]
fn the_real_resume_takes_over_a_task_the_loop_itself_left_failed() {
    let f = fixture();

    let (error, _) = resume_owned_by(&f, Process::current(&Probe));

    assert!(!error.contains("still running"), "{error}");
}

#[test]
fn the_real_resume_still_refuses_a_task_another_live_process_owns() {
    let f = fixture();
    let other = Process::new(std::process::id().wrapping_add(1), 0);

    let (error, rec) = resume_owned_by(&f, other.clone());

    assert!(error.contains("still running"), "{error}");
    assert_eq!(rec.process, Some(other));
}

/// Notes, at each phase, whether the loop had taken `#3`'s label off yet; in `run`, once the
/// record exists, it gives the loop a few seconds to notice while the phase is still running.
struct LabelWatch<'a> {
    script: Script,
    gh: &'a FakeGitHub,
    seen: RefCell<Vec<(String, bool)>>,
}

impl LabelWatch<'_> {
    fn removed(&self) -> bool {
        self.gh.calls().contains(&GhCall::IssueLabels(
            REPO.into(),
            3,
            Vec::new(),
            vec![READY.to_owned()],
        ))
    }

    fn note(&self, phase: &str) {
        let removed = self.removed();
        self.seen.borrow_mut().push((phase.to_owned(), removed));
    }
}

impl Steps for LabelWatch<'_> {
    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let result = self.script.run(issue, out, warn);
        if record::task_dir(&self.script.base, "issue-3")
            .join("task.json")
            .is_file()
        {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while !self.removed() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.note("run");
        result
    }
    fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.note("resume");
        self.script.resume(issue, out, warn)
    }
    fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.note("finish");
        self.script.finish(issue, out, warn)
    }
    fn file_findings(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.script.file_findings(issue, out, warn)
    }
    fn room(&self) -> Result<()> {
        self.script.room()
    }
}

fn watch_label(acts: &[Act]) -> Vec<(String, bool)> {
    let f = fixture();
    let gh = github(vec![labeled(3, "size:S")]);
    let steps = LabelWatch {
        script: Script::new(&f.base()).plays(3, acts),
        gh: &gh,
        seen: RefCell::default(),
    };
    let now = || steps.script.clock.get();
    let env = Env {
        now: &now,
        sleep: &|_| {},
        stop_requested: &|| false,
    };
    task_loop::run(
        &f.env.config_dir(),
        &with_cap(&f, 1),
        &steps,
        &gh,
        &Probe,
        &env,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    steps.seen.into_inner()
}

/// Review M-2: the label goes once the task's record exists, while its first phase still runs,
/// not after `task finish`.
#[test]
fn the_label_is_gone_while_the_first_phase_runs_and_before_finish() {
    let seen = watch_label(&[Act::Ready(0)]);

    assert_eq!(
        seen,
        [("run".to_owned(), true), ("finish".to_owned(), true)]
    );
}

#[test]
fn the_label_is_gone_before_a_usage_wait_s_resume() {
    let seen = watch_label(&[
        Act::Fail("Claude AI usage limit reached", true),
        Act::Ready(0),
    ]);

    assert_eq!(
        seen,
        [
            ("run".to_owned(), true),
            ("resume".to_owned(), true),
            ("finish".to_owned(), true)
        ]
    );
}
