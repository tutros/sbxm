//! Milestone 5 slice 2 (issue 600, decisions 317 and 321): the loop picks each task's location
//! from the issue's `run-cloud` / `run-local` labels, and `task fix --pr N` runs where the PR's
//! task ran unless `--cloud` or `--local` says otherwise.

mod common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};

use sbxm::backend::Location;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::location::{self, NEEDS_CLOUD_SLOTS, RUN_CLOUD_LABEL, RUN_LOCAL_LABEL};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

use common::task_fixture::{Probe, issue_text, open_issue};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

// --- the labels ---

#[test]
fn no_label_leaves_the_location_to_the_default() {
    assert_eq!(location::for_loop(&labels(&["size:S"]), None), Ok(None));
    assert_eq!(location::for_loop(&labels(&[]), Some(2)), Ok(None));
}

#[test]
fn run_local_is_local_even_with_cloud_slots_set() {
    assert_eq!(
        location::for_loop(&labels(&[RUN_LOCAL_LABEL]), Some(2)),
        Ok(Some(Location::Local))
    );
    assert_eq!(
        location::for_loop(&labels(&[RUN_LOCAL_LABEL]), None),
        Ok(Some(Location::Local))
    );
}

#[test]
fn run_cloud_with_cloud_slots_set_is_cloud() {
    assert_eq!(
        location::for_loop(&labels(&["size:S", RUN_CLOUD_LABEL]), Some(2)),
        Ok(Some(Location::Cloud))
    );
    assert_eq!(
        location::for_loop(&labels(&[RUN_CLOUD_LABEL]), Some(1)),
        Ok(Some(Location::Cloud))
    );
}

#[test]
fn run_cloud_with_cloud_slots_unset_is_a_skip_with_its_reason() {
    assert_eq!(
        location::for_loop(&labels(&[RUN_CLOUD_LABEL]), None),
        Err(NEEDS_CLOUD_SLOTS.to_owned())
    );
    assert_eq!(NEEDS_CLOUD_SLOTS, "run-cloud needs [resources] cloud_slots");
}

#[test]
fn both_labels_are_a_skip_naming_both() {
    for slots in [None, Some(2)] {
        let why =
            location::for_loop(&labels(&[RUN_LOCAL_LABEL, RUN_CLOUD_LABEL]), slots).unwrap_err();
        assert!(
            why.contains("run-cloud") && why.contains("run-local"),
            "{why}"
        );
    }
}

// --- a fix's location without a flag ---

/// Writes the record of `issue-41`, whose task opened PR 7 of `repo`, run at `at`.
fn opened_pr7(base: &std::path::Path, repo: &str, at: Location) {
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo,
            title: "",
            base: "main",
            branch: "issue-41",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 1),
    );
    rec.location = at;
    rec.pr = Some(format!("https://github.com/{repo}/pull/7"));
    let dir = record::task_dir(base, "issue-41");
    fs::create_dir_all(&dir).unwrap();
    record::write(&dir, &rec).unwrap();
}

fn pr7_closing(issues: Vec<u32>) -> sbxm::github::PrInfo {
    let mut pr = common::fix_task::pr7(&"a".repeat(40));
    pr.closing_issues = issues;
    pr
}

#[test]
fn a_fix_follows_the_location_its_pr_task_recorded() {
    for at in [Location::Cloud, Location::Local] {
        let env = common::Env::new();
        opened_pr7(&env.base_dir(), REPO, at);
        // The record decides: GitHub isn't asked.
        let gh = FakeGitHub::default();

        let found = location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap();

        assert_eq!(found, Some(at));
        assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    }
}

#[test]
fn a_record_of_another_repo_or_pr_does_not_count() {
    let env = common::Env::new();
    opened_pr7(&env.base_dir(), "o/other", Location::Cloud);
    let mut pr8 = pr7_closing(vec![]);
    pr8.number = 8;
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![]))
        .with_pr(pr8)
        .with_open_issues(vec![]);

    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        None
    );
    assert_eq!(
        location::of_pr(&env.base_dir(), "o/other", 8, &gh).unwrap(),
        None
    );
}

#[test]
fn with_no_record_a_fix_follows_its_issues_label() {
    let env = common::Env::new();
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![41]))
        .with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);

    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        Some(Location::Cloud)
    );

    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![41]))
        .with_open_issues(vec![open_issue(41, &[RUN_LOCAL_LABEL], "")]);
    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        Some(Location::Local)
    );
}

#[test]
fn with_no_record_and_no_label_a_fix_has_no_location_of_its_own() {
    let env = common::Env::new();
    for issue in [
        open_issue(41, &["size:S"], ""),
        // Both labels pin nothing: the default stands.
        open_issue(41, &[RUN_CLOUD_LABEL, RUN_LOCAL_LABEL], ""),
    ] {
        let gh = FakeGitHub::default()
            .with_pr(pr7_closing(vec![41]))
            .with_open_issues(vec![issue]);
        assert_eq!(
            location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
            None
        );
    }
    // A hand-made PR that closes no issue.
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![]))
        .with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);
    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        None
    );
}

// --- the loop ---

/// Records each `task run` the loop asks for with its location; it fails before any record, so
/// the loop moves on.
#[derive(Default)]
struct Script {
    calls: RefCell<Vec<String>>,
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("run of #{issue} without a location"))
    }
    fn run_at(
        &self,
        issue: u32,
        location: Option<Location>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(format!("run {issue} {location:?}"));
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

/// A test config with `cloud_slots` as given, and a checkout with `sbxm-task.toml`.
fn loop_fixture(cloud_slots: Option<u32>) -> (common::Env, PathBuf) {
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
    (env, repo_root)
}

fn loop_opts(repo_root: &std::path::Path, issues: &[u32]) -> Options {
    Options {
        repo_root: repo_root.to_owned(),
        issues: issues.to_vec(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    }
}

/// Issue 3, ready and sized, with `extra` labels.
fn ready_issue(extra: &[&str]) -> FakeGitHub {
    let mut names = vec![READY, "size:S"];
    names.extend_from_slice(extra);
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(issue_text(3))
        .with_open_issues(vec![open_issue(3, &names, "Why\n")])
}

/// Runs the loop over the labeled queue to its stop; the runs it asked for and its output.
fn run_loop(
    env: &common::Env,
    repo_root: &std::path::Path,
    gh: &FakeGitHub,
) -> (Vec<String>, String) {
    let script = Script::default();
    let loop_env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    task_loop::run(
        &env.config_dir(),
        &loop_opts(repo_root, &[]),
        &script,
        gh,
        &Probe,
        &loop_env,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    (script.calls.take(), String::from_utf8(out).unwrap())
}

#[test]
fn the_loop_runs_an_unlabeled_issue_where_it_ran_before() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[]));

    assert_eq!(calls, ["run 3 None"], "{out}");
}

#[test]
fn the_loop_runs_a_run_local_issue_locally_even_with_cloud_slots_set() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[RUN_LOCAL_LABEL]));

    assert_eq!(calls, ["run 3 Some(Local)"], "{out}");
}

#[test]
fn the_loop_runs_a_run_cloud_issue_in_the_cloud_with_cloud_slots_set() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert_eq!(calls, ["run 3 Some(Cloud)"], "{out}");
}

#[test]
fn the_loop_skips_a_run_cloud_issue_while_cloud_slots_is_unset_creating_nothing() {
    let (env, repo_root) = loop_fixture(None);

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert!(calls.is_empty(), "{calls:?}");
    assert!(
        out.contains("skipped #3 Fix 3: run-cloud needs [resources] cloud_slots\n"),
        "{out}"
    );
    // The report of the issues passed over says it too.
    assert!(
        out.contains("passed over:\n  #3 Fix 3: run-cloud needs [resources] cloud_slots\n"),
        "{out}"
    );
    assert!(!record::task_dir(&env.base_dir(), "issue-3").exists());
}

#[test]
fn the_loop_skips_an_issue_with_both_labels_naming_them() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (calls, out) = run_loop(
        &env,
        &repo_root,
        &ready_issue(&[RUN_CLOUD_LABEL, RUN_LOCAL_LABEL]),
    );

    assert!(calls.is_empty(), "{calls:?}");
    assert!(
        out.contains("skipped #3 Fix 3: labeled both run-cloud and run-local"),
        "{out}"
    );
    assert!(!record::task_dir(&env.base_dir(), "issue-3").exists());
}

// --- `task loop --check` ---

fn check(env: &common::Env, repo_root: &std::path::Path, gh: &FakeGitHub) -> (bool, String) {
    let mut out = Vec::new();
    let taken = task_loop::check(
        &env.config_dir(),
        &loop_opts(repo_root, &[3]),
        gh,
        &Probe,
        &mut out,
    )
    .unwrap();
    (taken, String::from_utf8(out).unwrap())
}

#[test]
fn check_says_a_run_cloud_issue_needs_cloud_slots() {
    let (env, repo_root) = loop_fixture(None);

    let (taken, out) = check(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert!(!taken, "{out}");
    assert_eq!(
        out,
        "#3 Fix 3: skip: run-cloud needs [resources] cloud_slots\n\
         command: none, no issue would be taken\n"
    );
}

#[test]
fn check_takes_a_run_cloud_issue_with_cloud_slots_set() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (taken, out) = check(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert!(taken, "{out}");
}

#[test]
fn check_says_both_labels_skip() {
    let (env, repo_root) = loop_fixture(Some(2));

    let (taken, out) = check(
        &env,
        &repo_root,
        &ready_issue(&[RUN_CLOUD_LABEL, RUN_LOCAL_LABEL]),
    );

    assert!(!taken, "{out}");
    assert!(
        out.contains("#3 Fix 3: skip: labeled both run-cloud and run-local"),
        "{out}"
    );
}

// --- `sbxm status`'s queued issues ---

#[test]
fn status_says_why_a_queued_run_cloud_issue_waits_while_cloud_slots_is_unset() {
    let env = common::Env::new();
    let gh = ready_issue(&[RUN_CLOUD_LABEL]);
    let repo = Ok(REPO.to_owned());

    let unset = sbxm::status::looping::fetch_checking(&gh, &repo, &env.base_dir(), None, None);
    let set = sbxm::status::looping::fetch_checking(&gh, &repo, &env.base_dir(), None, Some(2));

    assert_eq!(
        unset.blocked.get(&3).map(String::as_str),
        Some(NEEDS_CLOUD_SLOTS)
    );
    assert_eq!(set.blocked.get(&3), None, "{:?}", set.blocked);
}
