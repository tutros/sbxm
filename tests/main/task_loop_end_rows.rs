//! Issue 708: the loop without `[resources] cloud_slots` (serial) and the one with it (parallel)
//! end a task through one path, so the same stopped task writes the same log row in both: a task
//! stopped by a lapsed sign-in or a usage limit keeps its worker and reviewer times and its rounds.
//! The phases are scripted: each writes the task's record with a worker and a reviewer run, then
//! fails as the scenario says.

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
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{
    self, Agent, Kind, NewTask, Process, ProcessProbe, Record, ReviewResult, RunInfo, Stage, Status,
};

const REPO: &str = "o/r";
const ISSUE: u32 = 3;
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;

/// Every process is alive.
struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, _: u32) -> Option<u64> {
        Some(0)
    }
}

/// How every phase fails.
#[derive(Clone, Copy, Debug)]
enum Fail {
    SignIn,
    UsageLimit,
}

struct Script {
    base: PathBuf,
    fail: Fail,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
}

impl Script {
    fn new(base: &Path, fail: Fail) -> Self {
        Self {
            base: base.to_owned(),
            fail,
            calls: RefCell::default(),
            clock: Cell::new(START),
        }
    }

    fn phase(&self, call: String, issue: u32) -> Result<()> {
        self.clock.set(self.clock.get() + 60);
        self.calls.borrow_mut().push(call);
        write_record(&self.base, issue);
        Err(match self.fail {
            Fail::SignIn => sbxm::task::pipeline::NotSignedIn::new("Not authenticated to Docker")
                .then(vec![format!("sbxm task run --issue {issue}")])
                .into(),
            Fail::UsageLimit => anyhow!("You've hit your usage limit"),
        })
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.phase(format!("run {issue}"), issue)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.phase(format!("resume {issue}"), issue)
    }

    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of #{issue}"))
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
        Ok(())
    }
}

fn agent(name: &str, duration_s: u64) -> Agent {
    Agent {
        harness: "claude".into(),
        model: None,
        sandbox: format!("sbxm-{name}"),
        workspace: "w".into(),
        profile: None,
        cpus: None,
        memory: None,
        template: None,
        run: Some(RunInfo {
            status: "ok".into(),
            usage: serde_json::Value::Null,
            duration_s,
        }),
        session: None,
    }
}

/// `issue-<n>` at `working`, failed, after a worker run of 120 s and a reviewer run of 45 s in
/// review round 2.
fn write_record(base: &Path, issue: u32) {
    let dir = record::task_dir(base, &format!("issue-{issue}"));
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
    rec.stage = Stage::Working;
    rec.status = Status::Failed;
    rec.worker = Some(agent("worker", 120));
    rec.reviewer = Some(agent("reviewer", 45));
    rec.review = Some(ReviewResult {
        round: 2,
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
}

/// The loop's config, with `cloud_slots` set when given (the parallel loop).
fn fixture(cloud_slots: Option<u32>) -> (TestEnv, PathBuf) {
    let env = TestEnv::new();
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
    (env, repo_root)
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(issue_text(ISSUE))
        .with_open_issues(vec![open_issue(ISSUE, &["ready-for-worker", "size:S"], "")])
}

/// Runs one loop over issue 3 failing as `fail`; returns its log's row for the issue.
fn stopped_row(cloud_slots: Option<u32>, fail: Fail) -> String {
    let (env, repo_root) = fixture(cloud_slots);
    let script = Script::new(&env.base_dir(), fail);
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let stop = || false;
    let loop_env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &env.config_dir(),
        &Options {
            repo_root,
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        &script,
        &github(),
        &Alive,
        &loop_env,
        &mut out,
        &mut warn,
    );
    let said = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    assert!(result.is_err(), "{cloud_slots:?}: the loop goes on\n{said}");
    let logs = env.base_dir().join(".sbxm").join("loop-logs");
    let log: String = fs::read_dir(&logs)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    let rows: Vec<&str> = log.lines().filter(|l| l.starts_with("| 1 | #3 ")).collect();
    assert_eq!(rows.len(), 1, "{cloud_slots:?}\n{log}\n{said}");
    rows[0].to_owned()
}

#[test]
fn task_loop_end_rows_a_sign_in_stop_logs_the_same_row_with_and_without_cloud_slots() {
    let serial = stopped_row(None, Fail::SignIn);
    let parallel = stopped_row(Some(1), Fail::SignIn);

    assert_eq!(serial, parallel);
    assert!(
        serial.contains("| 120 / 45 | 2 | stopped at working: sign-in |"),
        "{serial}"
    );
}

#[test]
fn task_loop_end_rows_a_usage_limit_stop_logs_the_same_row_with_and_without_cloud_slots() {
    let serial = stopped_row(None, Fail::UsageLimit);
    let parallel = stopped_row(Some(1), Fail::UsageLimit);

    assert_eq!(serial, parallel);
    assert!(
        serial.contains("| 120 / 45 | 2 | stopped at working: usage limit |"),
        "{serial}"
    );
}
