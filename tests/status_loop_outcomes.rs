//! Issue 493: a `task loop` started with `--issue` records each issue's outcome (done, parked,
//! skipped, held) and the tasks counted toward `--cap` in its lock as it goes, and `sbxm status`
//! shows "task N of M", done, parked and pending from them, instead of inferring them from the
//! task records (PR 490, Q-1). A lock without them, as an older binary wrote it, is still read
//! from the records.
//!
//! The loop's phases are scripted: each `task run` and `task resume` writes the task's record as
//! running in the loop's process, as the real ones do, and takes the Loop section of `sbxm
//! status` and the lock as they stand at that moment.

mod common;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, LoopSource};
use sbxm::commands::task_loop::{self, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::run::results::format_timestamp;
use sbxm::task::queue::{IssueOutcome, LockFile, Outcome, Progress, lock_path};
use sbxm::task::record::{
    self, Kind, NewTask, Process, Record, ReviewResult, Stage, Stamp, Status,
};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;
/// A live process under [`Probe`]: a command run by hand holding a task's lock.
const HAND: u32 = 4242;

/// What one scripted `task run` or `task resume` does.
#[derive(Clone, Copy)]
enum Act {
    /// Reaches `ready`, clean.
    Ready,
    /// Reaches `ready`, and a live command run by hand then holds the task's lock, so the loop's
    /// claim before `task finish` is refused.
    ReadyHeld,
    /// `sbx` can't run, before any task record exists.
    NoSbx,
    /// Fails, leaving the record at `working`; with the loop's process when `true`, with none
    /// otherwise (as a resume that cleared it before a preflight failed leaves it).
    Fail(bool),
}

/// The Loop section and the lock as one phase found them.
struct Seen {
    call: String,
    section: String,
    lock: LockFile,
}

struct Script {
    base: PathBuf,
    acts: RefCell<HashMap<u32, VecDeque<Act>>>,
    /// On `run <a>`, the lock planted on `issue-<b>` is freed, for each `(a, b)`.
    unlock_on: Vec<(u32, u32)>,
    seen: RefCell<Vec<Seen>>,
}

fn me() -> Process {
    Process::current(&Probe)
}

fn task_dir(base: &Path, issue: u32) -> PathBuf {
    record::task_dir(base, &format!("issue-{issue}"))
}

fn read(base: &Path, issue: u32) -> Option<Record> {
    record::read(&task_dir(base, issue).join("task.json")).ok()
}

fn write(base: &Path, issue: u32, change: impl FnOnce(&mut Record)) {
    let mut rec = read(base, issue).unwrap_or_else(|| {
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
            T0,
            me(),
        )
    });
    change(&mut rec);
    record::write(&task_dir(base, issue), &rec).unwrap();
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            acts: RefCell::default(),
            unlock_on: Vec::new(),
            seen: RefCell::default(),
        }
    }

    fn plays(self, issue: u32, acts: &[Act]) -> Self {
        self.acts
            .borrow_mut()
            .insert(issue, acts.iter().copied().collect());
        self
    }

    fn unlocking(mut self, on_run_of: u32, held: u32) -> Self {
        self.unlock_on.push((on_run_of, held));
        self
    }

    fn look(&self, call: String) {
        let lock = fs::read_to_string(lock_path(&self.base)).unwrap();
        self.seen.borrow_mut().push(Seen {
            call,
            section: section(&self.base),
            lock: serde_json::from_str(&lock).unwrap(),
        });
    }

    /// What the loop's lock and `sbxm status` said during `call` ("run 2", "resume 1").
    fn at(&self, call: &str) -> (String, LockFile) {
        let seen = self.seen.borrow();
        let s = seen
            .iter()
            .find(|s| s.call == call)
            .unwrap_or_else(|| panic!("no {call} in {:?}", calls(&seen)));
        (s.section.clone(), s.lock.clone())
    }

    fn act(&self, what: &str, issue: u32) -> Result<()> {
        if what == "run" {
            for &(_, held) in self.unlock_on.iter().filter(|(on, _)| *on == issue) {
                let _ = fs::remove_file(task_dir(&self.base, held).join("lock.json"));
            }
        }
        let act = self
            .acts
            .borrow_mut()
            .get_mut(&issue)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Act::Ready);
        if !matches!(act, Act::NoSbx) {
            write(&self.base, issue, |r| {
                r.stage = Stage::Working;
                r.status = Status::Running;
                r.process = Some(me());
            });
        }
        self.look(format!("{what} {issue}"));
        match act {
            Act::Ready | Act::ReadyHeld => {
                write(&self.base, issue, |r| {
                    r.stage = Stage::Ready;
                    r.status = Status::Ok;
                    r.review = Some(ReviewResult {
                        round: 1,
                        ..ReviewResult::default()
                    });
                });
                if matches!(act, Act::ReadyHeld) {
                    let mut held = serde_json::to_value(Process::new(HAND, 0)).unwrap();
                    held["token"] = "hand".into();
                    fs::write(
                        task_dir(&self.base, issue).join("lock.json"),
                        held.to_string(),
                    )
                    .unwrap();
                }
                Ok(())
            }
            Act::NoSbx => Err(anyhow::Error::new(sbxm::headless::ExecUnavailable(
                "sbx exec failed".into(),
            ))),
            Act::Fail(keeps_process) => {
                write(&self.base, issue, |r| {
                    r.status = Status::Failed;
                    r.process = keeps_process.then(me);
                });
                Err(anyhow!("sbx stopped answering"))
            }
        }
    }
}

fn calls(seen: &[Seen]) -> Vec<&str> {
    seen.iter().map(|s| s.call.as_str()).collect()
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.act("run", issue)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.act("resume", issue)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.look(format!("finish {issue}"));
        write(&self.base, issue, |r| {
            r.stage = Stage::Finished;
            r.stages.push(Stamp {
                stage: Stage::Finished,
                at: format_timestamp(T0),
            });
            r.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        });
        Ok(())
    }

    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        anyhow::bail!("unexpected resolve of PR #{pr}")
    }

    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        anyhow::bail!("unexpected log-fix of PR #{pr}")
    }

    fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

/// GitHub's open issues, `Fix n` each, none labeled.
fn github(issues: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n));
    }
    gh.with_open_issues(
        issues
            .iter()
            .map(|&n| open_issue(n, &["size:S"], ""))
            .collect(),
    )
}

/// The Loop section of `sbxm status` for `base`, the footer left out.
fn section(base: &Path) -> String {
    let gh = FakeGitHub::default();
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let env = status::Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 200,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        &Probe,
        &env,
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let start = text.find("\nLoop:").unwrap_or_else(|| panic!("{text}"));
    let text = &text[start + 1..];
    let end = text.find("\n\n").map_or(text.len(), |e| e + 1);
    text[..end].to_owned()
}

struct Loop {
    _env: TestEnv,
    out: String,
    lock: PathBuf,
}

fn run_loop(script: &Script, env: TestEnv, issues: &[u32], cap: Option<u32>) -> Loop {
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let gh = github(issues);
    let now = || T0;
    let sleep = |_| {};
    let stop = || false;
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &env.config_dir(),
        &Options {
            repo_root,
            issues: issues.to_vec(),
            cap,
            repo: Some(REPO.into()),
            wait: false,
        },
        script,
        &gh,
        &Probe,
        &task_loop::Env {
            now: &now,
            sleep: &sleep,
            stop_requested: &stop,
        },
        &mut out,
        &mut warn,
    );
    let out = String::from_utf8_lossy(&out).into_owned();
    // Two parked tasks in a row stop the loop with an error; the tests look at what it recorded.
    if let Err(e) = &result
        && !format!("{e:#}").contains("two tasks parked in a row")
    {
        panic!("{e:#}\n{out}");
    }
    let lock = lock_path(&env.base_dir());
    Loop {
        _env: env,
        out,
        lock,
    }
}

fn outcomes(pairs: &[(u32, Outcome)]) -> Vec<IssueOutcome> {
    pairs
        .iter()
        .map(|&(issue, outcome)| IssueOutcome { issue, outcome })
        .collect()
}

// --- the three cases of PR 490's Q-1 ---

/// Q-1 case 1 of 3: a ready-stage claim held by a command run by hand queues a retry and isn't
/// counted, so with `--cap 1` the running issue is the first and only counted task.
#[test]
fn a_held_claim_is_not_counted_so_the_running_issue_is_task_1_of_1() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir()).plays(1, &[Act::ReadyHeld]);

    let run = run_loop(&script, env, &[1, 2, 3], Some(1));

    let (text, lock) = script.at("run 2");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 0,
            current: Some(2),
            outcomes: outcomes(&[(1, Outcome::Held)]),
        })
    );
    assert!(
        text.contains("  task: issue-2 (Fix 2) at working, task 1 of 1\n"),
        "{text}"
    );
    assert!(!text.contains("parked"), "{text}");
    assert!(text.contains("  pending: none\n"), "{text}");
    assert!(
        text.contains("  not run (--cap 1): #1 (Fix 1), #3\n"),
        "{text}"
    );
    assert!(run.out.contains("--cap 1 reached"), "{}", run.out);
    assert!(!run.lock.exists());
}

/// Q-1 case 2 of 3: two environment failures park an issue before its task record exists; it
/// uses a cap slot, so with `--cap 2` the running issue is the second and last.
#[test]
fn an_issue_parked_before_its_record_exists_counts_toward_the_cap() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir()).plays(1, &[Act::NoSbx, Act::NoSbx]);

    run_loop(&script, env, &[1, 2, 3], Some(2));

    let (text, lock) = script.at("run 2");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 1,
            current: Some(2),
            outcomes: outcomes(&[(1, Outcome::Parked)]),
        })
    );
    assert!(text.contains(", task 2 of 2\n"), "{text}");
    assert!(text.contains("  parked: #1\n"), "{text}");
    assert!(!text.contains("skipped"), "{text}");
    assert!(text.contains("  pending: none\n"), "{text}");
    assert!(text.contains("  not run (--cap 2): #3\n"), "{text}");
}

/// Q-1 case 3 of 3: a resume that cleared the loop's process from the record and then failed
/// leaves a parked task with no process; it is still shown parked and counted.
#[test]
fn a_task_parked_with_no_process_is_still_parked() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir()).plays(1, &[Act::Fail(true), Act::Fail(false)]);

    run_loop(&script, env, &[1, 2], None);

    let (text, lock) = script.at("run 2");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 1,
            current: Some(2),
            outcomes: outcomes(&[(1, Outcome::Parked)]),
        })
    );
    assert!(text.contains(", task 2 of 2\n"), "{text}");
    assert!(text.contains("  parked: #1 (Fix 1)\n"), "{text}");
    assert!(!text.contains("skipped"), "{text}");
}

// --- boundary cases (decision 244) ---

/// Boundary: `--cap` equal to the list's length runs every issue.
#[test]
fn a_cap_equal_to_the_list_runs_every_issue() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir());

    run_loop(&script, env, &[1, 2, 3], Some(3));

    let (text, lock) = script.at("run 3");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 2,
            current: Some(3),
            outcomes: outcomes(&[(1, Outcome::Done), (2, Outcome::Done)]),
        })
    );
    assert!(text.contains(", task 3 of 3\n"), "{text}");
    assert!(
        text.contains("  done: 2 (issue-1 (Fix 1), issue-2 (Fix 2))\n"),
        "{text}"
    );
    assert!(text.contains("  pending: none\n"), "{text}");
    assert!(!text.contains("not run"), "{text}");
}

/// Boundary: `--cap` one under the list's length leaves the last issue not run.
#[test]
fn a_cap_one_under_the_list_leaves_the_last_issue_not_run() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir());

    run_loop(&script, env, &[1, 2, 3], Some(2));

    let (text, _) = script.at("run 1");
    assert!(text.contains(", task 1 of 2\n"), "{text}");
    assert!(
        text.contains("  pending: 1 issue (1 blocked)\n    #2 — blocked: isn't an open issue\n"),
        "{text}"
    );
    assert!(text.contains("  not run (--cap 2): #3\n"), "{text}");
    let (text, _) = script.at("run 2");
    assert!(text.contains(", task 2 of 2\n"), "{text}");
    assert!(text.contains("  not run (--cap 2): #3\n"), "{text}");
    let runs = script
        .seen
        .borrow()
        .iter()
        .filter(|s| s.call.starts_with("run "))
        .count();
    assert_eq!(runs, 2);
}

/// Boundary: `--cap` one over the list's length is the list's length.
#[test]
fn a_cap_one_over_the_list_is_the_list() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir());

    run_loop(&script, env, &[1, 2, 3], Some(4));

    let (text, _) = script.at("run 1");
    assert!(text.contains(", task 1 of 3\n"), "{text}");
    assert!(
        text.contains(
            "  pending: 2 issues (2 blocked)\n    #2 — blocked: isn't an open issue\n    #3 — \
             blocked: isn't an open issue\n"
        ),
        "{text}"
    );
    let (text, _) = script.at("run 3");
    assert!(text.contains(", task 3 of 3\n"), "{text}");
    assert!(!text.contains("not run"), "{text}");
}

/// Boundary: an issue held, then tried again after the next task and done.
#[test]
fn an_issue_held_then_retried_and_done() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir())
        .plays(1, &[Act::ReadyHeld])
        .unlocking(2, 1);

    let run = run_loop(&script, env, &[1, 2, 3], None);

    let (text, lock) = script.at("run 2");
    assert_eq!(
        lock.progress.as_ref().map(|p| (p.counted, p.current)),
        Some((0, Some(2)))
    );
    assert!(text.contains(", task 1 of 3\n"), "{text}");
    assert!(
        text.contains("  held: #1 (Fix 1), tried again after the running task\n"),
        "{text}"
    );
    assert!(
        text.contains("  pending: 1 issue (1 blocked)\n    #3 — blocked: isn't an open issue\n"),
        "{text}"
    );
    let (text, lock) = script.at("run 3");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 2,
            current: Some(3),
            outcomes: outcomes(&[(2, Outcome::Done), (1, Outcome::Done)]),
        })
    );
    assert!(text.contains(", task 3 of 3\n"), "{text}");
    assert!(!text.contains("held"), "{text}");
    assert!(
        text.contains("  done: 2 (issue-1 (Fix 1), issue-2 (Fix 2))\n"),
        "{text}"
    );
    assert!(run.out.contains("tried again"), "{}", run.out);
}

/// Boundary: a lock from an older binary (the list and the cap, no outcomes) is read from the
/// task records as before: a record left by the loop's process short of finished is parked.
#[test]
fn a_lock_without_outcomes_is_read_from_the_records() {
    let env = TestEnv::new();
    let base = env.base_dir();
    let path = lock_path(&base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: me(),
        issues: vec![1, 2, 3],
        cap: None,
        progress: None,
    };
    fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
    write(&base, 1, |r| {
        r.stage = Stage::Working;
        r.status = Status::Failed;
        r.process = Some(me());
    });
    write(&base, 2, |r| {
        r.stage = Stage::Working;
        r.status = Status::Running;
        r.process = Some(me());
    });

    let text = section(&base);

    assert!(text.contains(", task 2 of 3\n"), "{text}");
    assert!(text.contains("  parked: #1 (Fix 1)\n"), "{text}");
    assert!(
        text.contains("  pending: 1 issue (1 blocked)\n    #3 — blocked: isn't an open issue\n"),
        "{text}"
    );
}

/// A recorded progress whose current issue has no task record yet still shows the issue and
/// its place in the list.
#[test]
fn a_current_issue_without_a_record_yet_is_shown_from_the_lock() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir()).plays(2, &[Act::NoSbx]);

    run_loop(&script, env, &[1, 2], None);

    let (text, _) = script.at("run 2");
    assert!(text.contains("  task: #2, task 2 of 2\n"), "{text}");
}

/// The lock is written whole each time: what an older binary reads of it is still its process.
#[test]
fn an_older_binary_reads_a_lock_with_outcomes_as_its_process() {
    let file = LockFile {
        process: me(),
        issues: vec![1, 2],
        cap: Some(1),
        progress: Some(Progress {
            counted: 1,
            current: Some(2),
            outcomes: outcomes(&[(1, Outcome::Parked)]),
        }),
    };
    let text = serde_json::to_string(&file).unwrap();

    let process: Process = serde_json::from_str(&text).unwrap();
    let again: LockFile = serde_json::from_str(&text).unwrap();

    assert_eq!(process, me());
    assert_eq!(again, file);
}

/// What the loop left in its lock before its first task: an empty progress, so a loop that has
/// done nothing yet is told from an older binary's lock.
#[test]
fn the_lock_records_an_empty_progress_before_the_first_task() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir());

    run_loop(&script, env, &[1], None);

    let (_, lock) = script.at("run 1");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 0,
            current: Some(1),
            outcomes: Vec::new(),
        })
    );
}

// --- review M-1: held issues fit under `--cap` in the order the loop tries them ---

/// Issues 1, 2 and 3 are held at their claims and 4 is done; 2 and 3 are freed, 1 stays held,
/// so its retry is held again and goes behind 2 and 3. During 2's finish, the one slot `--cap 3`
/// leaves goes to 3, the loop's next retry, and 1 is not run.
#[test]
fn a_retry_held_again_goes_behind_the_retries_already_due() {
    let env = TestEnv::new();
    let script = Script::new(&env.base_dir())
        .plays(1, &[Act::ReadyHeld])
        .plays(2, &[Act::ReadyHeld])
        .plays(3, &[Act::ReadyHeld])
        .unlocking(4, 2)
        .unlocking(4, 3);

    let run = run_loop(&script, env, &[1, 2, 3, 4], Some(3));

    let (text, lock) = script.at("finish 2");
    assert_eq!(
        lock.progress,
        Some(Progress {
            counted: 1,
            current: Some(2),
            outcomes: outcomes(&[(3, Outcome::Held), (4, Outcome::Done), (1, Outcome::Held)]),
        })
    );
    assert!(
        text.contains("  task: issue-2 (Fix 2) at ready, task 2 of 3\n"),
        "{text}"
    );
    assert!(
        text.contains("  held: #3 (Fix 3), tried again after the running task\n"),
        "{text}"
    );
    assert!(text.contains("  pending: none\n"), "{text}");
    assert!(text.contains("  not run (--cap 3): #1 (Fix 1)\n"), "{text}");
    assert!(
        run.out.contains("task 3: #3 Fix 3 (size S), tried again"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("#1 (Fix 1) was not tried again after its lock was held"),
        "{}",
        run.out
    );
}

/// An outcome recorded again moves to the end: the outcomes stay in the order the loop reached
/// them, which is the order held issues are tried again in.
#[test]
fn an_outcome_set_again_moves_to_the_end() {
    let mut progress = Progress::default();
    progress.set(1, Outcome::Held);
    progress.set(2, Outcome::Held);
    progress.set(1, Outcome::Held);

    assert_eq!(
        progress.outcomes,
        outcomes(&[(2, Outcome::Held), (1, Outcome::Held)])
    );
}
