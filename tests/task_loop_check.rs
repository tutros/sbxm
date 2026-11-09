//! Issue 511 (decision 307): `sbxm task loop --check` says for each issue whether the loop would
//! take it and prints the loop command, running nothing; and the loop itself passes over an
//! issue whose body still has a `## Open points` heading.

mod common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, Issue};
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

// --- a branch on origin, through `gh` ---

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    outputs: Mutex<Vec<String>>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        self.outputs
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow!("no scripted output left"))
    }
}

fn gh_answering(outputs: &[&str]) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn a_branch_on_origin_is_found_by_its_exact_ref() {
    // The shape of `gh api repos/<repo>/git/matching-refs/heads/<branch>` (written by hand, no
    // GitHub access where issue 511 was built): every ref the name starts, so only an exact
    // match counts.
    let (gh, script) = gh_answering(&[
        r#"[{"ref":"refs/heads/issue-468","object":{"sha":"abc","type":"commit"}}]"#,
        r#"[{"ref":"refs/heads/issue-4680","object":{"sha":"abc","type":"commit"}}]"#,
        "[]",
    ]);

    assert!(gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-468").unwrap());
    assert_eq!(
        script.calls.lock().unwrap()[0],
        ["api", "repos/o/r/git/matching-refs/heads/issue-468"]
    );
}

#[test]
fn a_bad_branch_name_is_refused_before_gh_runs() {
    let (gh, script) = gh_answering(&[]);
    assert!(gh.branch_exists(REPO, "../x").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn the_fake_has_the_branches_a_test_scripted() {
    let gh = FakeGitHub::default().with_branch_head("issue-468", "abc");
    assert!(gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-469").unwrap());
    assert_eq!(
        gh.calls()[0],
        GhCall::BranchExists(REPO.into(), "issue-468".into())
    );
}
