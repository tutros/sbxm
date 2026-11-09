//! Issue 511 (decision 307): `sbxm task loop --check` says for each issue whether the loop would
//! take it and prints the loop command, running nothing; and the loop itself passes over an
//! issue whose body still has a `## Open points` heading.

mod common;

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
use sbxm::task::queue::has_open_points;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const OPEN_POINTS: &str = "Why\n\n## Open points\n\n1. A or B?\n\n**Depends on:** none.\n";
const ANSWERED: &str = "Why\n\n## Answered (the user)\n\n1. A.\n\n**Depends on:** none.\n";

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

// --- the `## Open points` heading ---

#[test]
fn an_open_points_heading_is_found() {
    assert!(has_open_points("Why\n\n## Open points\n\n1. A or B?\n"));
    assert!(has_open_points("## Open points\n"));
    assert!(has_open_points("## Open points  \r\n- a?\n"));
}

#[test]
fn an_answered_heading_and_no_open_points_is_not_open() {
    assert!(!has_open_points("Why\n\n## Answered (the user)\n\n1. A.\n"));
    assert!(!has_open_points(""));
}

#[test]
fn open_points_in_a_code_block_or_not_at_a_line_start_do_not_count() {
    assert!(!has_open_points(
        "Example:\n\n```md\n## Open points\n```\n\n## Answered\n"
    ));
    assert!(!has_open_points("~~~\n## Open points\n~~~\n"));
    assert!(!has_open_points(
        "readied issues rename `## Open points` to `## Answered`\n"
    ));
    assert!(!has_open_points("  see ## Open points\n"));
    assert!(!has_open_points("### Open points\n"));
    // After a closed fence, a heading counts again.
    assert!(has_open_points("```\nx\n```\n## Open points\n"));
}

// --- the loop passes over an issue with open points ---

#[test]
fn the_loop_passes_over_a_labeled_issue_with_open_points_and_reports_it() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &[READY, "size:S"], OPEN_POINTS),
        open_issue(7, &[READY, "size:S"], ANSWERED),
    ]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert_eq!(*script.calls.borrow(), ["run 7"]);
    assert!(
        out.contains("skipped #3 Fix 3: open points not answered"),
        "{out}"
    );
    // The empty queue's report names it with the other passed-over issues.
    let report = out.split("queue is empty").nth(1).unwrap_or_default();
    assert!(
        report.contains("passed over:\n  #3 Fix 3: open points not answered"),
        "{out}"
    );
}

#[test]
fn the_loop_passes_over_a_named_issue_with_open_points() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &["size:S"], OPEN_POINTS)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[3]), &script, &gh);

    result.unwrap();
    assert!(script.calls.borrow().is_empty());
    assert!(
        out.contains("skipped #3 Fix 3: open points not answered"),
        "{out}"
    );
}
