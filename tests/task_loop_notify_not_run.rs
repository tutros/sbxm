//! Issue 555: `sbxm task loop` toasts a low-risk PR it opened that its not-run list holds back
//! from auto-merge (issue 545), read from the review that covered the PR's head through the same
//! check the arm paths use. The PRs are `FakeGitHub`'s and the notifier is `FakeNotifier`.

mod common;

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::Probe;
use sbxm::commands::task_loop::{self, Env, LOOP_HOOK, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::notify::{FakeNotifier, Notification};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const PUSHED: &str = "1111111111111111111111111111111111111111";
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A loop with nothing queued: every phase is unexpected.
struct Idle;

impl Steps for Idle {
    fn run(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of {issue}"))
    }

    fn resume(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of {issue}"))
    }

    fn file_findings(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected file-findings of {issue}"))
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

    fn review_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected review of PR #{pr}"))
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

fn review(commit: &str, risk: Level, not_run: Option<&[&str]>) -> ReviewResult {
    ReviewResult {
        round: 1,
        commit: Some(commit.to_owned()),
        must_fix: 0,
        risk,
        not_run: not_run.map(|checks| checks.iter().map(|&c| c.to_owned()).collect()),
        ..ReviewResult::default()
    }
}

/// A finished loop task `issue-3` whose review of [`HEAD`] is `rev`, opened PR #103.
fn loop_task(base: &Path, rev: ReviewResult) {
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 3,
            repo: REPO,
            title: "Fix 3",
            base: "main",
            branch: "sbxm/issue-3",
            config_hash: "h",
            id: None,
        },
        START,
        Process::new(1, 0),
    );
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.review = Some(rev);
    rec.pr = Some(format!("https://github.com/{REPO}/pull/103"));
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    record::write(&record::task_dir(base, "issue-3"), &rec).unwrap();
}

/// A finished fix task of PR #103, started at [`HEAD`], whose review is `rev`.
fn fix_task(base: &Path, rev: ReviewResult) {
    let id = record::fix_id(103, HEAD).unwrap();
    let mut fix = Record::new(
        &NewTask {
            kind: Kind::Fix,
            number: 103,
            repo: REPO,
            title: "Fix 3",
            base: "main",
            branch: "b103",
            config_hash: "h",
            id: Some(&id),
        },
        START,
        Process::new(1, 0),
    );
    fix.stage = Stage::Finished;
    fix.status = Status::Ok;
    fix.review = Some(rev);
    record::write(&record::task_dir(base, &id), &fix).unwrap();
}

fn github(head: &str) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN)
        .with_open_issues(Vec::new())
        .with_pr(PrInfo {
            number: 103,
            head_ref: "b103".into(),
            head_oid: head.into(),
            is_cross_repository: false,
            head_repo: String::new(),
            state: PrState::Open,
            title: "t".into(),
            body: String::new(),
            closing_issues: Vec::new(),
            base_moved: false,
        })
}

/// Runs the idle loop once against PR #103 at `head` and returns what it showed.
fn notified(env: &TestEnv, head: &str) -> Vec<Notification> {
    let repo_root = env.tmp.path().join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let opts = Options {
        repo_root,
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let now = || START;
    let env_ = Env {
        now: &now,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let notifier = FakeNotifier::default();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run_notifying(
        &env.config_dir(),
        &opts,
        &Idle,
        &github(head),
        &Probe,
        &env_,
        &notifier,
        &mut out,
        &mut warn,
    )
    .unwrap();
    notifier.shown()
}

const TITLE: &str = "PR #103 awaits your merge (checks not run)";

#[test]
fn a_low_risk_pr_with_no_check_left_not_run_stays_quiet() {
    let env = TestEnv::new();
    loop_task(&env.base_dir(), review(HEAD, Level::Low, Some(&[])));

    assert!(notified(&env, HEAD).is_empty());
}

#[test]
fn a_low_risk_pr_with_one_check_not_run_notifies_naming_it() {
    let env = TestEnv::new();
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Low, Some(&["the real sbx test"])),
    );

    let shown = notified(&env, HEAD);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, TITLE);
    assert_eq!(
        shown[0].body,
        "1 check(s) not run: the real sbx test; next: run them, then merge"
    );
    assert_eq!(shown[0].key, format!("pr:103:{HEAD}"));
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
}

#[test]
fn a_low_risk_pr_with_several_checks_not_run_gives_the_count_and_the_first() {
    let env = TestEnv::new();
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Low, Some(&["first check", "second", "third"])),
    );

    let shown = notified(&env, HEAD);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, TITLE);
    assert_eq!(
        shown[0].body,
        "3 check(s) not run: first check; next: run them, then merge"
    );
}

#[test]
fn a_low_risk_pr_with_an_unknown_not_run_list_notifies_unknown() {
    let env = TestEnv::new();
    loop_task(&env.base_dir(), review(HEAD, Level::Low, None));

    let shown = notified(&env, HEAD);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, TITLE);
    assert_eq!(
        shown[0].body,
        "the review's not-run list is unknown; next: read the review, then merge"
    );
}

#[test]
fn a_medium_risk_pr_with_checks_not_run_keeps_the_medium_toast() {
    let env = TestEnv::new();
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Medium, Some(&["the real sbx test"])),
    );

    let shown = notified(&env, HEAD);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, "PR #103 awaits your merge (medium)");
    assert!(shown[0].body.contains("risk medium"), "{}", shown[0].body);
}

#[test]
fn the_same_pr_and_head_seen_again_notifies_once() {
    let env = TestEnv::new();
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Low, Some(&["the real sbx test"])),
    );

    assert_eq!(notified(&env, HEAD).len(), 1);
    let again = notified(&env, HEAD);

    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn the_not_run_list_is_read_from_the_review_that_covered_the_head() {
    let env = TestEnv::new();
    // The opening review left a check; the fix's review of the pushed head ran everything.
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Low, Some(&["old check"])),
    );
    fix_task(&env.base_dir(), review(PUSHED, Level::Low, Some(&[])));

    assert!(notified(&env, PUSHED).is_empty());

    // And the other way round: a fix's review that left a check notifies with it.
    let env = TestEnv::new();
    loop_task(&env.base_dir(), review(HEAD, Level::Low, Some(&[])));
    fix_task(
        &env.base_dir(),
        review(PUSHED, Level::Low, Some(&["new check"])),
    );

    let shown = notified(&env, PUSHED);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0].body,
        "1 check(s) not run: new check; next: run them, then merge"
    );
    assert_eq!(shown[0].key, format!("pr:103:{PUSHED}"));
}

#[test]
fn a_low_risk_pr_with_checks_not_run_notifies_while_the_loop_is_paused() {
    let env = TestEnv::new();
    loop_task(
        &env.base_dir(),
        review(HEAD, Level::Low, Some(&["the real sbx test"])),
    );
    // A pause file that can't be read counts as a pause.
    let sbxm = env.base_dir().join(".sbxm");
    std::fs::create_dir_all(&sbxm).unwrap();
    std::fs::write(sbxm.join("loop-pause.json"), "not json").unwrap();

    let shown = notified(&env, HEAD);

    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, TITLE);
}

#[test]
fn a_head_no_review_covered_stays_quiet() {
    // Review M-1: the PR moved past its only review; that review's list says nothing of the new
    // head, so the PR waits for a review of it, not on a toast.
    for not_run in [Some(&["old check"][..]), None] {
        let env = TestEnv::new();
        loop_task(&env.base_dir(), review(HEAD, Level::Low, not_run));

        let shown = notified(&env, PUSHED);

        assert!(shown.is_empty(), "{shown:?}");
    }
}
