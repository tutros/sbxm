//! Issue 709: one rule decides an issue's size, `queue::size`. It gives the size, or an error for
//! none and for more than one; `task loop --check` names a doubly-sized issue as blocked, the
//! loop never takes it, and a medium-risk PR's size reason reads the same labels.

use crate::common;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, ready_open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::Issue;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::medium;
use sbxm::task::queue::{self, SizeError};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";

fn labels(list: &[&str]) -> Vec<String> {
    list.iter().map(|l| (*l).to_owned()).collect()
}

// --- the rule ---

#[test]
fn task_queue_size_of_no_size_label_is_an_error() {
    assert_eq!(queue::size(&labels(&[])), Err(SizeError::Missing));
    assert_eq!(queue::size(&labels(&[READY])), Err(SizeError::Missing));
    assert_eq!(
        SizeError::Missing.to_string(),
        "no size label (size:S, size:M or size:L)"
    );
}

#[test]
fn task_queue_size_of_exactly_one_size_label_is_that_size() {
    assert_eq!(queue::size(&labels(&[READY, "size:S"])), Ok("S"));
    assert_eq!(queue::size(&labels(&["size:M"])), Ok("M"));
    assert_eq!(queue::size(&labels(&["size:L", READY])), Ok("L"));
}

#[test]
fn task_queue_size_of_two_different_size_labels_is_an_error_in_either_order() {
    let several = SizeError::Several(vec!["S", "L"]);
    assert_eq!(queue::size(&labels(&["size:S", "size:L"])), Err(several));
    assert_eq!(
        queue::size(&labels(&["size:L", READY, "size:S"])),
        Err(SizeError::Several(vec!["L", "S"]))
    );
    assert_eq!(
        SizeError::Several(vec!["S", "L"]).to_string(),
        "more than one size label (S, L)"
    );
}

#[test]
fn task_queue_size_of_the_same_size_label_twice_is_that_size() {
    assert_eq!(queue::size(&labels(&["size:M", "size:M"])), Ok("M"));
}

// --- `task loop --check` and the loop ---

/// `task run` fails before any record, so the loop moves on; records the issues it ran.
#[derive(Default)]
struct Script {
    calls: Mutex<Vec<u32>>,
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.lock().unwrap().push(issue);
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

fn doubly_sized() -> FakeGitHub {
    github(vec![
        ready_open_issue(3, &[READY, "size:S", "size:L"], ""),
        ready_open_issue(7, &[READY, "size:M"], ""),
    ])
}

/// Runs `task loop --check`; whether every issue would be taken, and its output.
fn check(f: &Fixture, issues: &[u32], gh: &FakeGitHub) -> (Result<bool>, String) {
    let mut out = Vec::new();
    let result = task_loop::check(&f.env.config_dir(), &opts(f, issues), gh, &Probe, &mut out);
    (result, String::from_utf8(out).unwrap())
}

/// Runs the loop to its stop; the issues it ran and its output.
fn run_loop(f: &Fixture, issues: &[u32], gh: &FakeGitHub) -> (Vec<u32>, String) {
    let script = Script::default();
    let env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    task_loop::run(
        &f.env.config_dir(),
        &opts(f, issues),
        &script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    let calls = script.calls.lock().unwrap().clone();
    (calls, String::from_utf8(out).unwrap())
}

#[test]
fn task_queue_size_check_names_a_doubly_sized_issue_as_blocked() {
    let f = fixture();

    let (result, out) = check(&f, &[3, 7], &doubly_sized());

    assert!(!result.unwrap(), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "#3 Fix 3: skip: more than one size label (S, L)",
        "{out}"
    );
    assert_eq!(lines[1], "#7 Fix 7: take", "{out}");
}

#[test]
fn task_queue_size_the_queue_never_takes_a_doubly_sized_issue() {
    let f = fixture();

    let (calls, out) = run_loop(&f, &[], &doubly_sized());

    assert_eq!(calls, [7], "{out}");
    assert!(
        out.contains("skipped #3 Fix 3: more than one size label (S, L)"),
        "{out}"
    );
}

#[test]
fn task_queue_size_the_loop_never_runs_a_named_doubly_sized_issue() {
    let f = fixture();

    let (calls, out) = run_loop(&f, &[3], &doubly_sized());

    assert!(calls.is_empty(), "{out}");
    assert!(
        out.contains("skipped #3 Fix 3: more than one size label (S, L)"),
        "{out}"
    );
}

// --- the empty queue's report ---

#[test]
fn task_queue_size_an_almost_ready_issue_with_two_sizes_names_them_as_a_blocker() {
    let open = vec![ready_open_issue(9, &["size:S", "size:L"], "")];
    let text = queue::report(&queue::Report {
        open: &open,
        skipped: &[],
    });

    assert!(
        text.contains("  #9 Fix 9 (size:?)\n    - more than one size label (S, L)\n"),
        "{text}"
    );
}

// --- a medium-risk PR's size, read from its own issue (review M-1) ---

/// PR 120 of issue 20 changing one source file, with 200 newer open issues and issue 20 itself
/// not among them: older than the listing holds, or closed while its PR is open. Its labels are
/// read from the issue itself, `labels`.
fn issue_outside_the_listing(labels: &[&str]) -> FakeGitHub {
    let newer = (21..=220)
        .map(|n| ready_open_issue(n, &["size:S"], ""))
        .collect();
    FakeGitHub::default()
        .with_open_issues(newer)
        .with_issue_label_names(20, labels)
        .with_pr_files(
            120,
            sbxm::github::PrFiles {
                changed_files: 1,
                files: vec![sbxm::github::PrFile {
                    filename: "src/a.rs".to_owned(),
                    previous_filename: None,
                }],
            },
        )
}

fn medium_reasons(gh: &FakeGitHub) -> Vec<String> {
    medium::refusals(gh, REPO, medium::Source::Issue(20), 120)
}

#[test]
fn task_queue_size_a_medium_pr_of_a_size_s_or_m_issue_outside_the_open_listing_passes() {
    assert_eq!(
        medium_reasons(&issue_outside_the_listing(&["size:S"])),
        Vec::<String>::new()
    );
    assert_eq!(
        medium_reasons(&issue_outside_the_listing(&[READY, "size:M"])),
        Vec::<String>::new()
    );
}

#[test]
fn task_queue_size_a_medium_pr_of_an_issue_outside_the_open_listing_keeps_the_size_rule() {
    assert_eq!(
        medium_reasons(&issue_outside_the_listing(&[])),
        ["its issue #20 has no size label"]
    );
    assert_eq!(
        medium_reasons(&issue_outside_the_listing(&["size:L"])),
        ["its issue #20 is size L"]
    );
    assert_eq!(
        medium_reasons(&issue_outside_the_listing(&["size:S", "size:M"])),
        ["its issue #20 has more than one size label (S, M)"]
    );
}

#[test]
fn task_queue_size_a_medium_pr_whose_issue_labels_cant_be_read_is_refused() {
    let gh = issue_outside_the_listing(&["size:S"]).failing_issue_label_reads("gh failed");
    let got = medium_reasons(&gh);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("its issue #20's labels could not be read: gh failed"),
        "{got:?}"
    );
}
