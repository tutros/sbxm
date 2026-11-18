//! Issue 725: the loop runs the readiness rules (`src/task/ready.rs`) before it starts an issue.
//! One that fails a rule is skipped with the rule's reason, in the loop's output and its empty
//! queue's report as in `task loop --check`; a warn never holds an issue back.

use crate::common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::Issue;
use sbxm::github::fake::FakeGitHub;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// A body every readiness rule passes.
const READY_BODY: &str =
    "**Size:** S\n\n**Depends on:** none\n\n## Acceptance criteria\n\n- [ ] It works\n";
/// The same with no `**Depends on:**` line and no acceptance criteria.
const UNREADY_BODY: &str = "**Size:** S\n\nDo it.\n";
const NO_DEPENDS: &str = "no **Depends on:** line";
const NO_ACCEPTANCE: &str = "no Acceptance criteria heading";

/// Records the phases the loop asks for; `task run` fails before any record, so the loop moves on.
#[derive(Default)]
struct Script {
    calls: RefCell<Vec<String>>,
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        Err(anyhow!("scripted refusal"))
    }
    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of #{issue}"))
    }
    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of #{issue}"))
    }
    fn file_findings(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected file-findings of #{issue}"))
    }
    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }
    fn fix_check(
        &self,
        pr: u32,
        _: u64,
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

fn opts(f: &Fixture, issues: &[u32]) -> Options {
    Options {
        repo_root: f.repo_root.clone(),
        issues: issues.to_vec(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    }
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

/// Runs the loop to its stop; its output.
fn run_loop(f: &Fixture, opts: &Options, script: &Script, gh: &FakeGitHub) -> (Result<()>, String) {
    let env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let result = task_loop::run(
        &f.env.config_dir(),
        opts,
        script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

fn check(f: &Fixture, issues: &[u32], gh: &FakeGitHub) -> (Result<bool>, String) {
    let mut out = Vec::new();
    let result = task_loop::check(&f.env.config_dir(), &opts(f, issues), gh, &Probe, &mut out);
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn task_loop_ready_a_labeled_issue_that_fails_a_rule_is_skipped_with_its_reason() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &[READY, "size:S"], UNREADY_BODY),
        open_issue(7, &[READY, "size:S"], READY_BODY),
    ]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert_eq!(*script.calls.borrow(), ["run 7"], "{out}");
    let skipped = out
        .lines()
        .find(|l| l.starts_with("skipped #3 Fix 3: "))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        skipped.contains(NO_DEPENDS) && skipped.contains(NO_ACCEPTANCE),
        "{out}"
    );
    // The empty queue's report names it with the other passed-over issues.
    let report = out.split("queue is empty").nth(1).unwrap_or_default();
    assert!(report.contains("passed over:\n  #3 Fix 3: "), "{out}");
    assert!(report.contains(NO_DEPENDS), "{out}");
}

#[test]
fn task_loop_ready_a_named_issue_that_fails_a_rule_is_skipped() {
    let f = fixture();
    // A `**Size:**` line that names another size than its label.
    let body = READY_BODY.replace("**Size:** S", "**Size:** L");
    let gh = github(vec![open_issue(3, &["size:S"], &body)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[3]), &script, &gh);

    result.unwrap();
    assert!(script.calls.borrow().is_empty(), "{out}");
    assert!(
        out.contains("skipped #3 Fix 3: labeled size:S but its **Size:** line says L"),
        "{out}"
    );
}

#[test]
fn task_loop_ready_an_unsized_issue_is_skipped() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &[READY], READY_BODY)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert!(script.calls.borrow().is_empty(), "{out}");
    assert!(out.contains("skipped #3 Fix 3: no size label"), "{out}");
}

#[test]
fn task_loop_ready_a_warn_never_holds_an_issue_back() {
    let f = fixture();
    // A path that isn't on the default branch and a malformed `**Files:**` line: two warns.
    let body = format!("{READY_BODY}\nChange `src/gone.rs`.\n\n**Files:** src/gone.rs\n");
    let gh = github(vec![open_issue(3, &[READY, "size:S"], &body)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert_eq!(*script.calls.borrow(), ["run 3"], "{out}");
    assert!(!out.contains("skipped #3 Fix 3:"), "{out}");
}

#[test]
fn task_loop_ready_check_gives_the_same_reason_and_takes_a_ready_issue() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &[READY, "size:S"], UNREADY_BODY),
        open_issue(7, &[READY, "size:S"], READY_BODY),
    ]);

    let (result, out) = check(&f, &[], &gh);

    assert!(!result.unwrap(), "{out}");
    let line = out
        .lines()
        .find(|l| l.starts_with("#3 Fix 3: skip: "))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains(NO_DEPENDS), "{out}");
    assert!(line.contains(&format!("; also: {NO_ACCEPTANCE}")), "{out}");
    assert!(out.contains("#7 Fix 7: take"), "{out}");
    assert!(out.contains("command: sbxm task loop --issue 7"), "{out}");
}
