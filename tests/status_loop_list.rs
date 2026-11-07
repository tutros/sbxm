//! Issue 477: a `task loop` started with `--issue` writes its list (and its `--cap`) into its
//! lock, and `sbxm status`'s Loop section shows where the loop is in that list: "task N of M",
//! the issues done, parked or skipped, and the ones pending in order, each with its title,
//! instead of the labeled queue. A loop without the list, or a lock an older binary wrote, is
//! shown as before.

mod common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use common::Env as TestEnv;
use common::task_fixture::{issue_text, open_issue};
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, LoopSource};
use sbxm::commands::task_loop::{self, Options, Steps};
use sbxm::github::Issue;
use sbxm::github::fake::FakeGitHub;
use sbxm::run::results::format_timestamp;
use sbxm::task::queue::{LOOP_HOOK, LockFile, lock_path};
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, ReviewResult, Stage, Stamp, Status,
};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;
/// When the loop in the fixtures started.
const LOOP_START: u64 = T0 - 1_000;

struct Probe(u64);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(self.0)
    }
}

fn loop_process() -> Process {
    Process::new(1, LOOP_START)
}

// --- the loop writes its list ---

/// `task run` and `task finish` as records only, keeping a copy of the loop's lock as each
/// `task run` found it.
struct Script {
    base: PathBuf,
    locks: RefCell<Vec<String>>,
}

impl Script {
    fn write(&self, issue: u32, stage: Stage) {
        let id = format!("issue-{issue}");
        let dir = record::task_dir(&self.base, &id);
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
                T0,
                Process::new(1, 0),
            )
        });
        rec.stage = stage;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            ..ReviewResult::default()
        });
        if stage == Stage::Finished {
            rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        }
        record::write(&dir, &rec).unwrap();
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.locks
            .borrow_mut()
            .push(fs::read_to_string(lock_path(&self.base)).unwrap());
        self.write(issue, Stage::Ready);
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.write(issue, Stage::Ready);
        Ok(())
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.write(issue, Stage::Finished);
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

/// Runs a loop over `issues` (the labeled queue when empty) and returns the lock as each task's
/// `task run` saw it.
fn loop_locks(issues: &[u32], cap: Option<u32>, labeled: &[u32]) -> (Vec<String>, PathBuf) {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    let mut open: Vec<Issue> = Vec::new();
    for &n in issues.iter().chain(labeled) {
        gh = gh.with_issue_text(issue_text(n));
        let labels: &[&str] = if labeled.contains(&n) {
            &["ready-for-worker", "size:S"]
        } else {
            &["size:S"]
        };
        open.push(open_issue(n, labels, ""));
    }
    let gh = gh.with_open_issues(open);
    let script = Script {
        base: env.base_dir(),
        locks: RefCell::default(),
    };
    let now = || T0;
    let sleep = |_| {};
    let stop = || false;
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run(
        &env.config_dir(),
        &Options {
            repo_root,
            issues: issues.to_vec(),
            cap,
            repo: Some(REPO.into()),
        },
        &script,
        &gh,
        &Probe(0),
        &task_loop::Env {
            now: &now,
            sleep: &sleep,
            stop_requested: &stop,
        },
        &mut out,
        &mut warn,
    )
    .unwrap_or_else(|e| panic!("{e:#}\n{}", String::from_utf8_lossy(&out)));
    let lock = lock_path(&env.base_dir());
    (script.locks.into_inner(), lock)
}

#[test]
fn a_loop_with_issues_writes_its_list_in_order_and_its_cap_into_the_lock_before_the_first_task() {
    let (locks, path) = loop_locks(&[469, 467, 468], Some(2), &[]);

    assert_eq!(locks.len(), 2, "{locks:?}");
    let file: LockFile = serde_json::from_str(&locks[0]).unwrap();
    assert_eq!(file.issues, vec![469, 467, 468]);
    assert_eq!(file.cap, Some(2));
    assert_eq!(file.process.pid, std::process::id());
    assert!(!path.exists(), "the loop removes its lock when it stops");
}

#[test]
fn a_loop_over_the_labeled_queue_writes_its_process_alone() {
    let (locks, _) = loop_locks(&[], Some(1), &[5]);

    assert_eq!(locks.len(), 1);
    let value: serde_json::Value = serde_json::from_str(&locks[0]).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["pid", "started_at"], "{value}");
}

#[test]
fn an_older_binary_reads_the_new_lock_as_its_process() {
    let file = LockFile {
        process: loop_process(),
        issues: vec![1, 2],
        cap: Some(1),
    };
    let text = serde_json::to_string(&file).unwrap();

    let process: Process = serde_json::from_str(&text).unwrap();

    assert_eq!(process, loop_process());
}

// --- `sbxm status` shows it ---

fn write_lock(base: &Path, issues: &[u32], cap: Option<u32>) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: loop_process(),
        issues: issues.to_vec(),
        cap,
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
}

fn task(number: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: REPO,
            title: &format!("title {number}"),
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, rec: &Record) {
    record::write(&record::task_dir(base, &rec.id), rec).unwrap();
}

fn running(base: &Path, number: u32) {
    let mut rec = task(number);
    rec.stage = Stage::Working;
    rec.status = Status::Running;
    rec.process = Some(loop_process());
    save(base, &rec);
}

fn finished(base: &Path, number: u32) {
    let mut rec = task(number);
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.stages.push(Stamp {
        stage: Stage::Finished,
        at: format_timestamp(LOOP_START + 10),
    });
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + number));
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    save(base, &rec);
}

/// Stopped short of finished by the loop's own process: what a parked task leaves.
fn parked(base: &Path, number: u32) {
    let mut rec = task(number);
    rec.stage = Stage::Working;
    rec.status = Status::Failed;
    rec.process = Some(loop_process());
    save(base, &rec);
}

/// GitHub's open issues: `Open n` for each, one of them labeled for the queue.
fn github(open: &[u32]) -> FakeGitHub {
    let mut issues: Vec<Issue> = open
        .iter()
        .map(|&n| Issue {
            number: n,
            open: true,
            title: format!("Open {n}"),
            labels: vec!["size:S".into()],
            body: String::new(),
        })
        .collect();
    issues.push(Issue {
        number: 900,
        open: true,
        title: "Labeled".into(),
        labels: vec!["ready-for-worker".into(), "size:S".into()],
        body: String::new(),
    });
    FakeGitHub::default().with_open_issues(issues)
}

fn frame(base: &Path, gh: &FakeGitHub) -> String {
    let source = LoopSource {
        github: gh,
        repo: Ok(REPO.into()),
    };
    let env = status::Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 200,
        iterations: None,
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
        &Probe(LOOP_START),
        &env,
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let start = text.find("\nLoop:").unwrap_or_else(|| panic!("{text}"));
    text[start + 1..].to_owned()
}

/// The Loop section's lines, the footer left out.
fn section(base: &Path, gh: &FakeGitHub) -> String {
    let text = frame(base, gh);
    let end = text.find("\n\n").map_or(text.len(), |e| e + 1);
    text[..end].to_owned()
}

#[test]
fn five_issues_two_done_and_the_third_running() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[469, 467, 468, 444, 455], None);
    finished(&b, 469);
    finished(&b, 467);
    running(&b, 468);

    let text = section(&b, &github(&[468, 444, 455]));

    assert!(
        text.contains("  task: issue-468 (title 468) at working, task 3 of 5\n"),
        "{text}"
    );
    assert!(
        text.contains("  done: 2 (issue-467 (title 467), issue-469 (title 469))\n"),
        "{text}"
    );
    assert!(
        text.contains("  pending: 2 issues\n    #444: Open 444\n    #455: Open 455\n"),
        "{text}"
    );
    assert!(!text.contains("queue"), "{text}");
    assert!(!text.contains("Labeled"), "{text}");
}

#[test]
fn one_issue_given_and_running() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[7], None);
    running(&b, 7);

    let text = section(&b, &github(&[7]));

    assert!(text.contains(", task 1 of 1\n"), "{text}");
    assert!(text.contains("  pending: none\n"), "{text}");
}

#[test]
fn the_last_issue_running_is_task_m_of_m() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2, 3], None);
    finished(&b, 1);
    finished(&b, 2);
    running(&b, 3);

    let text = section(&b, &github(&[3]));

    assert!(text.contains(", task 3 of 3\n"), "{text}");
    assert!(text.contains("  pending: none\n"), "{text}");
}

#[test]
fn every_issue_done_and_the_loop_stopping() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2], None);
    finished(&b, 1);
    finished(&b, 2);

    let text = section(&b, &github(&[]));

    assert!(
        text.contains("  task: none running, 2 of 2 run\n"),
        "{text}"
    );
    assert!(text.contains("  pending: none\n"), "{text}");
}

#[test]
fn nothing_started_yet_shows_the_whole_list_pending() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2], None);

    let text = section(&b, &github(&[1, 2]));

    assert!(
        text.contains("  task: none running, 0 of 2 run\n"),
        "{text}"
    );
    assert!(
        text.contains("  pending: 2 issues\n    #1: Open 1\n    #2: Open 2\n"),
        "{text}"
    );
}

#[test]
fn a_skipped_issue_and_a_parked_one_are_never_pending() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2, 3, 4], None);
    parked(&b, 1);
    // 2 has no record: refused before any write, or waiting on a dependency.
    running(&b, 3);

    let text = section(&b, &github(&[1, 2, 3, 4]));

    assert!(text.contains("  parked: #1 (title 1)\n"), "{text}");
    assert!(text.contains("  skipped: #2 (Open 2)\n"), "{text}");
    assert!(text.contains(", task 2 of 3\n"), "{text}");
    assert!(
        text.contains("  pending: 1 issue\n    #4: Open 4\n"),
        "{text}"
    );
}

#[test]
fn a_cap_smaller_than_the_list_counts_only_the_tasks_it_runs() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2, 3, 4, 5], Some(3));
    finished(&b, 1);
    running(&b, 2);

    let text = section(&b, &github(&[2, 3, 4, 5]));

    assert!(text.contains(", task 2 of 3\n"), "{text}");
    assert!(
        text.contains("  pending: 1 issue\n    #3: Open 3\n"),
        "{text}"
    );
    assert!(
        text.contains("  not run (--cap 3): #4 (Open 4), #5 (Open 5)\n"),
        "{text}"
    );
}

#[test]
fn a_lock_without_a_list_is_shown_as_before() {
    let base = TestEnv::new();
    let b = base.base_dir();
    // As an older binary, or a loop over the labeled queue, writes it.
    let path = lock_path(&b);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_string(&loop_process()).unwrap()).unwrap();
    running(&b, 3);

    let text = section(&b, &github(&[3]));

    assert!(text.contains("Loop: running (process 1, "), "{text}");
    assert!(
        text.contains("  task: issue-3 (title 3) at working\n"),
        "{text}"
    );
    assert!(
        text.contains("  queue: 1 issue\n    #900: Labeled (size S)\n"),
        "{text}"
    );
    assert!(!text.contains(", task "), "{text}");
    assert!(!text.contains(" run\n"), "{text}");
    assert!(!text.contains("pending"), "{text}");
}

// --- review M-1: a failed GitHub read stays visible ---

#[test]
fn a_failed_github_read_is_said_for_a_listed_loop_and_its_progress_still_shows() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[1, 2], None);
    running(&b, 1);

    let text = section(&b, &FakeGitHub::default().failing("HTTP 502: bad gateway"));

    assert!(
        text.contains("  issue titles: unreadable (HTTP 502: bad gateway)\n"),
        "{text}"
    );
    assert!(
        text.contains("issue-1 (title 1) at working, task 1 of 2\n"),
        "{text}"
    );
    assert!(text.contains("  pending: 1 issue\n    #2\n"), "{text}");
    assert!(!text.contains("queue"), "{text}");
}
