//! Issue 725: the loop runs the readiness rules (`src/task/ready.rs`) before it starts an issue.
//! One that fails a rule is skipped with the rule's reason, in the loop's output and its empty
//! queue's report as in `task loop --check`; a warn never holds an issue back, and both say it
//! (review M-2).

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
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, LabelEvent, MergedPr, PrFile, PrFiles};

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
    // Both warns are said before the start (review M-2).
    let task = out.find("task 1: #3").unwrap_or_else(|| panic!("{out}"));
    let paths = out
        .find("#3 Fix 3: warn paths: ")
        .unwrap_or_else(|| panic!("{out}"));
    assert!(paths < task, "{out}");
    assert!(out.contains("#3 Fix 3: warn files: "), "{out}");
}

/// 2025-11-10T12:00:00Z, when the issue was last labeled.
const LABELED_AT: u64 = 1_762_776_000;

/// GitHub where ready issue #7 names `src/cli.rs`, was labeled at [`LABELED_AT`], and PR #21
/// changed `src/cli.rs` an hour after that.
fn stale_github() -> FakeGitHub {
    let body = format!("{READY_BODY}\nChange `src/cli.rs`.\n");
    let gh = github(vec![open_issue(7, &[READY, "size:S"], &body)])
        .with_repo_files(&[("src/cli.rs", "")])
        .with_merged_prs(vec![MergedPr {
            number: 21,
            merged_at: LABELED_AT + 3_600,
        }])
        .with_pr_files(
            21,
            PrFiles {
                changed_files: 1,
                files: vec![PrFile {
                    filename: "src/cli.rs".into(),
                    previous_filename: None,
                }],
            },
        );
    gh.set_label_events(
        7,
        vec![LabelEvent {
            label: READY.into(),
            added: true,
            at: LABELED_AT,
        }],
    );
    gh
}

const STALE: &str = "#21 (src/cli.rs)";

/// Whether `gh` made the reads of the paths and staleness rules for issue 7.
fn read_paths_and_staleness(gh: &FakeGitHub) -> bool {
    let calls = gh.calls();
    let has = |f: &dyn Fn(&GhCall) -> bool| calls.iter().any(f);
    has(&|c| matches!(c, GhCall::RepoPaths(..)))
        && has(&|c| matches!(c, GhCall::LabelEvents(_, 7)))
        && has(&|c| matches!(c, GhCall::PrsMergedSince(..)))
        && has(&|c| matches!(c, GhCall::PrFiles(_, 21)))
}

#[test]
fn task_loop_ready_the_loop_says_a_stale_issue_before_it_starts_it() {
    let f = fixture();
    let gh = stale_github();
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert_eq!(*script.calls.borrow(), ["run 7"], "{out}");
    let line = out
        .lines()
        .find(|l| l.starts_with("#7 Fix 7: warn staleness: "))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains(STALE), "{out}");
    assert!(read_paths_and_staleness(&gh), "{:?}", gh.calls());
}

#[test]
fn task_loop_ready_check_says_the_warns_of_an_issue_it_would_take() {
    let f = fixture();
    let gh = stale_github();

    let (result, out) = check(&f, &[], &gh);

    assert!(result.unwrap(), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    let take = lines
        .iter()
        .position(|l| *l == "#7 Fix 7: take")
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        lines[take + 1].starts_with("  warn staleness: ") && lines[take + 1].contains(STALE),
        "{out}"
    );
    assert!(read_paths_and_staleness(&gh), "{:?}", gh.calls());
}

#[test]
fn task_loop_ready_check_says_paths_and_files_warns_without_skipping() {
    let f = fixture();
    let body = format!("{READY_BODY}\nChange `src/gone.rs`.\n\n**Files:** src/gone.rs\n");
    let gh = github(vec![open_issue(3, &[READY, "size:S"], &body)])
        .with_repo_files(&[("src/cli.rs", "")]);

    let (result, out) = check(&f, &[], &gh);

    assert!(result.unwrap(), "{out}");
    assert!(out.contains("#3 Fix 3: take\n  warn paths: "), "{out}");
    assert!(out.contains("\n  warn files: "), "{out}");
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

#[test]
fn task_loop_ready_a_queued_run_job_says_its_warns_before_it_starts() {
    let f = fixture();
    let gh = stale_github();
    let queue = sbxm::task::jobs::queue_dir(&f.env.base_dir());
    fs::create_dir_all(&queue).unwrap();
    fs::write(queue.join("r.json"), r#"{"kind": "run", "issue": 7}"#).unwrap();
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    // Said once the job is taken and before its task runs (here to `task run`'s refusal).
    let taken = out
        .find("job run #7: started")
        .unwrap_or_else(|| panic!("{out}"));
    let ran = out
        .find("job run #7: refused")
        .unwrap_or_else(|| panic!("{out}"));
    let warned = out[taken..ran].contains("#7 Fix 7: warn staleness: ");
    assert!(warned, "{out}");
}
