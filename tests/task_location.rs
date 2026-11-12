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

use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_fix;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
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

// --- `task fix --pr N` ---
//
// Both pools are full and refuse at once: a fix that would run in the cloud is refused by the
// cloud slot, one that would run locally by the sandbox slot, each before it creates anything.

/// PR 7 at a new head, its latest review by the user posted, closing `closing`.
fn fix_github(f: &common::task_fixture::Fixture, closing: Vec<u32>) -> FakeGitHub {
    let head = common::fix_task::new_head(f);
    let mut pr = common::fix_task::pr7(&head);
    pr.closing_issues = closing;
    FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(pr)
        .with_pr_comments(7, vec![common::fix_task::posted_review(&head)])
}

fn full_pools(f: &common::task_fixture::Fixture) -> sbxm::disk::Guard {
    let _ = common::slot::full(&f.env, true);
    common::cloud_slot::full(&f.env, true)
}

fn cloud_ready_backend() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"]).with_version(common::cloud_task::CLOUD_MIN)
}

/// `task fix --pr 7` with `flag` as `--cloud`/`--local`; the error it ends with.
fn fix(f: &common::task_fixture::Fixture, flag: Option<Location>, gh: &FakeGitHub) -> String {
    let backend = cloud_ready_backend();
    let result = task_fix::run(
        &f.env.config_dir(),
        &task_fix::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some(REPO.into()),
            base: None,
            clone_source: Some(common::task_fixture::source(f)),
            identity: Some(common::cloud_task::identity()),
            disk: full_pools(f),
            check_run: None,
            location: flag,
        },
        &backend,
        gh,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    format!("{:#}", result.unwrap_err())
}

fn assert_ran_in_the_cloud(err: &str) {
    assert!(err.starts_with(&common::cloud_slot::refusal()), "{err}");
}

fn assert_ran_locally(err: &str) {
    assert!(err.starts_with(&common::slot::refusal()), "{err}");
}

#[test]
fn a_fix_of_a_cloud_tasks_pr_runs_in_the_cloud_and_of_a_local_ones_locally() {
    let f = common::fix_task::config();
    opened_pr7(&f.env.base_dir(), REPO, Location::Cloud);
    assert_ran_in_the_cloud(&fix(&f, None, &fix_github(&f, vec![])));

    let f = common::fix_task::config();
    opened_pr7(&f.env.base_dir(), REPO, Location::Local);
    // The record wins over the issue's label.
    let gh =
        fix_github(&f, vec![41]).with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);
    assert_ran_locally(&fix(&f, None, &gh));
}

#[test]
fn a_fix_with_no_task_record_follows_its_issues_label() {
    let f = common::fix_task::config();
    let gh =
        fix_github(&f, vec![41]).with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);
    assert_ran_in_the_cloud(&fix(&f, None, &gh));

    let f = common::fix_task::config();
    let gh = fix_github(&f, vec![41]).with_open_issues(vec![open_issue(41, &["size:S"], "")]);
    assert_ran_locally(&fix(&f, None, &gh));
}

#[test]
fn a_flag_wins_over_the_pr_tasks_location() {
    let f = common::fix_task::config();
    opened_pr7(&f.env.base_dir(), REPO, Location::Cloud);
    assert_ran_locally(&fix(&f, Some(Location::Local), &fix_github(&f, vec![])));

    let f = common::fix_task::config();
    opened_pr7(&f.env.base_dir(), REPO, Location::Local);
    assert_ran_in_the_cloud(&fix(&f, Some(Location::Cloud), &fix_github(&f, vec![])));
}

/// The loop's resolve (and log-fix) of a PR goes through `task fix` with no flag.
#[test]
fn the_loops_resolve_of_a_cloud_tasks_pr_runs_in_the_cloud() {
    let f = common::fix_task::config();
    opened_pr7(&f.env.base_dir(), REPO, Location::Cloud);
    let gh = fix_github(&f, vec![]);
    let backend = cloud_ready_backend();
    let host = FakeHostRunner::default();
    let steps = task_loop::TaskSteps {
        config_dir: &f.env.config_dir(),
        repo_root: f.env.tmp.path().join("target-repo"),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: full_pools(&f),
        stop: None,
        spawner: &common::child::NoChild::default(),
    };

    let err = steps
        .fix_pr(7, &mut Vec::new(), &mut Vec::new())
        .unwrap_err();

    // A local fix is refused by the full sandbox slot first thing. The loop's own identity is
    // the user's git config, read just before the cloud slot: where git has none, that read is
    // the refusal instead.
    let err = format!("{err:#}");
    assert!(
        err.starts_with(&common::cloud_slot::refusal()) || err.contains("git has no user."),
        "{err}"
    );
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
}

#[test]
fn task_fix_cloud_and_local_conflict() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["task", "fix", "--pr", "7", "--cloud", "--local"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("'--cloud' cannot be used with '--local'"),
        "{stderr}"
    );
}

// --- a queued `run` job ---

#[test]
fn a_queued_run_job_of_a_run_cloud_issue_runs_in_the_cloud() {
    let (env, repo_root) = loop_fixture(Some(2));
    let queue = sbxm::task::jobs::queue_dir(&env.base_dir());
    fs::create_dir_all(&queue).unwrap();
    fs::write(queue.join("r.json"), r#"{"kind": "run", "issue": 3}"#).unwrap();

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    // The job first; the queue then tries the issue again, still labeled.
    assert_eq!(
        calls.first().map(String::as_str),
        Some("run 3 Some(Cloud)"),
        "{out}"
    );
}

#[test]
fn a_queued_run_job_of_a_run_cloud_issue_is_refused_while_cloud_slots_is_unset() {
    let (env, repo_root) = loop_fixture(None);
    let queue = sbxm::task::jobs::queue_dir(&env.base_dir());
    fs::create_dir_all(&queue).unwrap();
    fs::write(queue.join("r.json"), r#"{"kind": "run", "issue": 3}"#).unwrap();

    let (calls, out) = run_loop(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert!(calls.is_empty(), "{calls:?}");
    let done = sbxm::task::jobs::done_dir(&env.base_dir()).join("r.json");
    let result = fs::read_to_string(&done).unwrap();
    assert!(result.contains("\"refused\""), "{result}\n{out}");
    assert!(result.contains(NEEDS_CLOUD_SLOTS), "{result}");
}

// --- review of issue 600, M-2: only the task that opened the PR decides a fix's location ---

/// Writes a record under `dir` naming PR 7 of [`REPO`] as its `pr`: kind `kind`, run at `at`,
/// and with `continues` the PR it continued.
fn named_pr7(dir: &std::path::Path, kind: Kind, id: &str, at: Location, continues: bool) {
    let mut rec = Record::new(
        &NewTask {
            kind,
            number: if kind == Kind::Fix { 7 } else { 41 },
            repo: REPO,
            title: "",
            base: "main",
            branch: "feature-x",
            config_hash: "h",
            id: Some(id),
        },
        0,
        Process::new(1, 1),
    );
    rec.location = at;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/7"));
    if continues {
        rec.continues = Some(record::PrBranch {
            pr: 7,
            branch: "feature-x".into(),
            base: "a".repeat(40),
        });
    }
    fs::create_dir_all(dir).unwrap();
    record::write(dir, &rec).unwrap();
}

const FIX_ID: &str = "fix-7-aaaaaaaaaaaa";

fn labeled_41(label: &str) -> FakeGitHub {
    FakeGitHub::default()
        .with_pr(pr7_closing(vec![41]))
        .with_open_issues(vec![open_issue(41, &[label], "")])
}

#[test]
fn a_previous_fix_is_not_the_task_that_opened_the_pr() {
    for (fix_at, label, expected) in [
        (Location::Local, RUN_CLOUD_LABEL, Location::Cloud),
        (Location::Cloud, RUN_LOCAL_LABEL, Location::Local),
    ] {
        let env = common::Env::new();
        let base = env.base_dir();
        named_pr7(
            &record::task_dir(&base, FIX_ID),
            Kind::Fix,
            FIX_ID,
            fix_at,
            false,
        );

        assert_eq!(location::recorded(&base, REPO, 7), None);
        assert_eq!(
            location::of_pr(&base, REPO, 7, &labeled_41(label)).unwrap(),
            Some(expected)
        );
    }
}

#[test]
fn a_task_that_continued_the_pr_is_not_the_one_that_opened_it() {
    let env = common::Env::new();
    let base = env.base_dir();
    named_pr7(
        &record::task_dir(&base, "issue-41"),
        Kind::Issue,
        "issue-41",
        Location::Local,
        true,
    );

    assert_eq!(location::recorded(&base, REPO, 7), None);
    assert_eq!(
        location::of_pr(&base, REPO, 7, &labeled_41(RUN_CLOUD_LABEL)).unwrap(),
        Some(Location::Cloud)
    );
}

#[test]
fn the_opener_wins_over_a_later_fix_live_or_archived() {
    for archived in [false, true] {
        let env = common::Env::new();
        let base = env.base_dir();
        // Ids on both sides of the opener's, so no listing order can put the fix first only.
        for id in ["fix-7-000000000000", FIX_ID, "fix-7-ffffffffffff"] {
            named_pr7(
                &record::task_dir(&base, id),
                Kind::Fix,
                id,
                Location::Local,
                false,
            );
        }
        let opener = if archived {
            sbxm::task::merged::archive_root(&base).join("issue-41")
        } else {
            record::task_dir(&base, "issue-41")
        };
        named_pr7(&opener, Kind::Issue, "issue-41", Location::Cloud, false);

        assert_eq!(
            location::recorded(&base, REPO, 7),
            Some(Location::Cloud),
            "archived: {archived}"
        );
    }
}

// --- review of issue 600, M-1: the loop's disk wait is for the step's own location ---
//
// The room is `TaskSteps`'s own check, on a sandbox drive with 0 bytes free under the repo's
// local default; a loop that pauses is stopped at its first look back.

/// Plays the phases as [`Script`] does, and `task fix --pr N` as `fix N`, failing; the room is
/// `real`'s.
struct LowDisk<'a> {
    calls: RefCell<Vec<String>>,
    real: task_loop::TaskSteps<'a>,
}

impl Steps for LowDisk<'_> {
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
        self.calls.borrow_mut().push(format!("fix {pr}"));
        Err(anyhow!("scripted fix failure"))
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
        self.real.room()
    }
    fn room_for(&self, location: Option<Location>) -> Result<()> {
        self.real.room_for(location)
    }
}

/// Runs the loop over the labeled queue with [`LowDisk`]: its calls and output. The first sleep
/// (a pause) asks it to stop.
fn run_low_disk(
    env: &common::Env,
    repo_root: &std::path::Path,
    gh: &FakeGitHub,
) -> (Vec<String>, String) {
    let state = env.tmp.path().join("sbx-state");
    fs::create_dir_all(&state).unwrap();
    let backend = FakeBackend::default();
    let host = FakeHostRunner::default();
    let config_dir = env.config_dir();
    let steps = LowDisk {
        calls: RefCell::new(Vec::new()),
        real: task_loop::TaskSteps {
            config_dir: &config_dir,
            repo_root: repo_root.to_owned(),
            repo: Some(REPO.into()),
            backend: &backend,
            github: gh,
            probe: &Probe,
            host: &host,
            disk: sbxm::disk::Guard {
                free_space: |_| Ok(0),
                default_state_dir: Some(state),
                ..Default::default()
            },
            stop: None,
            spawner: &common::child::NoChild::default(),
        },
    };
    let slept = std::cell::Cell::new(false);
    let loop_env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| slept.set(true),
        stop_requested: &|| slept.get(),
    };
    let mut out = Vec::new();
    task_loop::run(
        &config_dir,
        &loop_opts(repo_root, &[]),
        &steps,
        gh,
        &Probe,
        &loop_env,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    (steps.calls.take(), String::from_utf8(out).unwrap())
}

#[test]
fn a_run_cloud_issue_does_not_wait_for_the_local_disk_and_a_run_local_one_does() {
    let (env, repo_root) = loop_fixture(Some(2));
    let (calls, out) = run_low_disk(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));
    assert_eq!(calls, ["run 3 Some(Cloud)"], "{out}");
    assert!(!out.contains("paused:"), "{out}");

    let (env, repo_root) = loop_fixture(Some(2));
    let (calls, out) = run_low_disk(&env, &repo_root, &ready_issue(&[RUN_LOCAL_LABEL]));
    assert!(calls.is_empty(), "{calls:?}");
    assert!(out.contains("paused: only 0.0 GiB free"), "{out}");
}

#[test]
fn a_queued_run_job_of_a_run_cloud_issue_does_not_wait_for_the_local_disk() {
    let (env, repo_root) = loop_fixture(Some(2));
    let queue = sbxm::task::jobs::queue_dir(&env.base_dir());
    fs::create_dir_all(&queue).unwrap();
    fs::write(queue.join("r.json"), r#"{"kind": "run", "issue": 3}"#).unwrap();

    let (calls, out) = run_low_disk(&env, &repo_root, &ready_issue(&[RUN_CLOUD_LABEL]));

    assert_eq!(
        calls.first().map(String::as_str),
        Some("run 3 Some(Cloud)"),
        "{out}"
    );
    assert!(!out.contains("paused:"), "{out}");
}

/// A finished loop task of issue 1, run at `at`, that opened PR 101; GitHub reports PR 101
/// conflicting with its base.
fn conflicting_loop_pr(env: &common::Env, at: Location) -> FakeGitHub {
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 1,
            repo: REPO,
            title: "",
            base: "main",
            branch: "issue-1",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 1),
    );
    rec.location = at;
    rec.stage = record::Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/101"));
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    let dir = record::task_dir(&env.base_dir(), "issue-1");
    fs::create_dir_all(&dir).unwrap();
    record::write(&dir, &rec).unwrap();
    let mut pr = common::fix_task::pr7(&"0".repeat(40));
    pr.number = 101;
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", &"a".repeat(40))
        .with_pr(pr)
        .with_mergeable(101, sbxm::github::Mergeable::Conflicting)
        .with_open_issues(vec![])
}

#[test]
fn the_resolve_of_a_cloud_tasks_pr_does_not_wait_for_the_local_disk_and_a_local_ones_does() {
    let (env, repo_root) = loop_fixture(Some(2));
    let gh = conflicting_loop_pr(&env, Location::Cloud);
    let (calls, out) = run_low_disk(&env, &repo_root, &gh);
    assert_eq!(calls, ["fix 101"], "{out}");

    let (env, repo_root) = loop_fixture(Some(2));
    let gh = conflicting_loop_pr(&env, Location::Local);
    let (calls, out) = run_low_disk(&env, &repo_root, &gh);
    assert!(calls.is_empty(), "{calls:?}\n{out}");
}
