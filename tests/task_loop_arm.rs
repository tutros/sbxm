//! Issue 375 (decision 293): `sbxm task loop` arms GitHub's auto-merge on a low-risk PR it
//! opened, when the in-task review covered the PR's head with 0 must-fix, and cleans up a merged
//! PR's task with `task rm --merged`. The phases are scripted; the PRs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{PrFile, PrFiles, PrInfo, PrState};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
/// The commit the in-task review covered.
const REVIEWED: &str = "0123456789abcdef0123456789abcdef01234567";
const MOVED: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// What the scripted review of every task says.
#[derive(Clone, Copy)]
struct Review {
    risk: Level,
    must_fix: u32,
    /// The commit the review covered.
    commit: &'static str,
    /// The checks it couldn't run (issue 545); `None` when its list is unknown.
    not_run: Option<&'static [&'static str]>,
}

const LOW_CLEAN: Review = Review {
    risk: Level::Low,
    must_fix: 0,
    commit: REVIEWED,
    not_run: Some(&[]),
};

/// Plays `task run`, `task finish`, `task file-findings` and `task rm --merged`.
struct Script {
    base: PathBuf,
    review: Review,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    rm_fails: bool,
}

impl Script {
    fn new(base: &Path, review: Review) -> Self {
        Self {
            base: base.to_owned(),
            review,
            calls: RefCell::default(),
            clock: Cell::new(START),
            rm_fails: false,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        self.clock.set(self.clock.get() + 300);
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = Record::new(
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
        );
        rec.stage = Stage::Ready;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            commit: Some(self.review.commit.to_owned()),
            must_fix: self.review.must_fix,
            risk: self.review.risk,
            not_run: self
                .review
                .not_run
                .map(|checks| checks.iter().map(|c| (*c).to_owned()).collect()),
            ..ReviewResult::default()
        });
        record::write(&dir, &rec).unwrap();
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("finish {issue}"));
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec).unwrap();
        Ok(())
    }

    fn file_findings(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(format!("file-findings {issue}"));
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }

    fn fix_check(
        &self,
        pr: u32,
        run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr} from run {run}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push("rm-merged".to_owned());
        if self.rm_fails {
            return Err(anyhow!("a task folder is locked"));
        }
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

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn log_text(&self) -> String {
        fs::read_to_string(
            self.base()
                .join(".sbxm/loop-logs/2026-11-02-100000-loop.md"),
        )
        .unwrap_or_default()
    }

    fn opts(&self, cap: Option<u32>) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap,
            repo: Some(REPO.into()),
            wait: false,
        }
    }
}

/// The PR task `issue` finishes into is `100 + issue`, open with its head at `head`.
fn pr(number: u32, state: PrState, head: &str) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

fn github(issues: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    let mut open = Vec::new();
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n));
        open.push(open_issue(n, &[READY, "size:S"], ""));
    }
    gh.with_open_issues(open)
}

struct Run {
    result: Result<()>,
    out: String,
    warn: String,
    sleeps: usize,
}

fn run_loop(f: &Fixture, opts: &Options, script: &Script, gh: &FakeGitHub) -> Run {
    let sleeps = Cell::new(0usize);
    let now = || script.clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        script.clock.set(script.clock.get() + d.as_secs());
    };
    // A paused loop is stopped by the user at its first look back.
    let stop = || sleeps.get() > 0;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        opts,
        script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        sleeps: sleeps.get(),
    }
}

fn arms(gh: &FakeGitHub) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrArmAutoMerge(..)))
        .collect()
}

#[test]
fn a_low_risk_pr_reviewed_at_its_head_is_armed_with_that_commit() {
    let f = fixture();
    let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
    let script = Script::new(&f.base(), LOW_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        arms(&gh),
        [GhCall::PrArmAutoMerge(REPO.into(), 103, REVIEWED.into())]
    );
    assert!(run.out.contains("auto-merge armed"), "{}", run.out);
    assert!(
        f.log_text().contains("PR #103, clean, auto-merge armed"),
        "{}",
        f.log_text()
    );
}

#[test]
fn an_unknown_risk_is_not_armed() {
    let f = fixture();
    let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
    let script = Script::new(
        &f.base(),
        Review {
            risk: Level::Unknown,
            ..LOW_CLEAN
        },
    );

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

// --- Issue 545: the checks a review couldn't run ---

mod not_run {
    use super::*;

    fn low_with(not_run: Option<&'static [&'static str]>) -> Review {
        Review {
            not_run,
            ..LOW_CLEAN
        }
    }

    /// Runs the loop on issue 3 with `review`; returns the run and the PR's arm calls.
    fn go(review: Review) -> (Run, Vec<GhCall>, String) {
        let f = fixture();
        let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
        let script = Script::new(&f.base(), review);
        let run = run_loop(&f, &f.opts(None), &script, &gh);
        let log = f.log_text();
        (run, arms(&gh), log)
    }

    #[test]
    fn low_risk_with_none_not_run_is_armed() {
        let (run, arms, _) = go(low_with(Some(&[])));
        run.result.unwrap();
        assert_eq!(
            arms,
            [GhCall::PrArmAutoMerge(REPO.into(), 103, REVIEWED.into())]
        );
    }

    #[test]
    fn low_risk_with_one_check_not_run_is_not_armed_and_says_so() {
        let (run, arms, log) = go(low_with(Some(&["Pester tests: no pwsh"])));
        run.result.unwrap();
        assert!(arms.is_empty(), "{}", run.out);
        assert!(
            run.out
                .contains("PR #103 (Fix 3) awaits you: auto-merge not armed: 1 check not run"),
            "{}",
            run.out
        );
        assert!(log.contains("not armed: 1 check not run"), "{log}");
    }

    #[test]
    fn several_checks_not_run_are_counted() {
        let (run, arms, log) = go(low_with(Some(&["a: x", "b: y"])));
        run.result.unwrap();
        assert!(arms.is_empty(), "{}", run.out);
        assert!(log.contains("not armed: 2 checks not run"), "{log}");
    }

    #[test]
    fn low_risk_with_an_unknown_list_is_not_armed_and_says_so() {
        let (run, arms, log) = go(low_with(None));
        run.result.unwrap();
        assert!(arms.is_empty(), "{}", run.out);
        assert!(
            run.out
                .contains("awaits you: auto-merge not armed: not-run list unknown"),
            "{}",
            run.out
        );
        assert!(log.contains("not armed: not-run list unknown"), "{log}");
    }

    /// A record from before the list has no `not_run` key: the same file an unknown list
    /// writes, so it is read back as unknown and not armed.
    #[test]
    fn a_record_from_before_the_list_is_not_armed() {
        let f = fixture();
        let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
        let script = Script::new(&f.base(), low_with(None));

        let run = run_loop(&f, &f.opts(None), &script, &gh);

        run.result.unwrap();
        let file = record::task_dir(&f.base(), "issue-3").join("task.json");
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert!(json["review"].get("not_run").is_none(), "{json}");
        assert!(arms(&gh).is_empty(), "{}", run.out);
    }

    /// Review S-1: a refusal that comes first (risk, a draft) doesn't hide the not-run reason.
    #[test]
    fn a_higher_risk_or_draft_pr_still_names_its_checks_not_run() {
        let cases = [
            (
                Level::Medium,
                0,
                Some(&["Pester tests: no pwsh"][..]),
                "1 check not run",
                "risk medium",
            ),
            (Level::High, 0, None, "not-run list unknown", "risk high"),
            (
                Level::Medium,
                0,
                None,
                "not-run list unknown",
                "risk medium",
            ),
            (
                Level::Low,
                1,
                Some(&["a: x", "b: y"][..]),
                "2 checks not run",
                "a draft",
            ),
        ];
        for (risk, must_fix, not_run, said, other) in cases {
            let (run, arms, log) = go(Review {
                risk,
                must_fix,
                ..low_with(not_run)
            });
            run.result.unwrap();
            assert!(arms.is_empty(), "{risk:?}: {}", run.out);
            let line = format!("awaits you: auto-merge not armed: {said}; {other}");
            assert!(run.out.contains(&line), "{line}\n{}", run.out);
            // A draft's log row is its work stop's, with no arming note.
            if must_fix == 0 {
                assert!(
                    log.contains(&format!("not armed: {said}")),
                    "{risk:?}: {log}"
                );
            }
        }
    }

    #[test]
    fn a_medium_risk_pr_with_none_not_run_still_waits_for_its_risk() {
        let (run, arms, _) = go(Review {
            risk: Level::Medium,
            ..low_with(Some(&[]))
        });
        run.result.unwrap();
        assert!(arms.is_empty(), "{}", run.out);
        assert!(run.out.contains("awaits you: risk medium"), "{}", run.out);
    }
}

#[test]
fn a_medium_or_high_risk_pr_waits_for_the_user_and_counts_toward_the_cap() {
    for risk in [Level::Medium, Level::High] {
        let f = fixture();
        // Four PRs of earlier tasks await the user, so this one is the fifth.
        let mut gh = github(&[3, 4]).with_pr(pr(103, PrState::Open, REVIEWED));
        for i in 0..4u32 {
            let issue = 50 + i;
            let script = Script::new(&f.base(), LOW_CLEAN);
            script.run(issue, &mut Vec::new(), &mut Vec::new()).unwrap();
            script
                .finish(issue, &mut Vec::new(), &mut Vec::new())
                .unwrap();
            let dir = record::task_dir(&f.base(), &format!("issue-{issue}"));
            let mut rec = record::read(&dir.join("task.json")).unwrap();
            rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
            record::write(&dir, &rec).unwrap();
            gh = gh.with_pr(pr(100 + issue, PrState::Open, REVIEWED));
        }
        let script = Script::new(&f.base(), Review { risk, ..LOW_CLEAN });

        let run = run_loop(&f, &f.opts(None), &script, &gh);

        run.result.unwrap();
        assert!(arms(&gh).is_empty(), "{risk:?}");
        // The fifth PR pauses the loop before issue 4: #4 never ran.
        assert!(run.sleeps > 0, "{risk:?}: {}", run.out);
        assert!(run.out.contains("5 PRs await you"), "{}", run.out);
        assert!(!script.calls().contains(&"run 4".to_owned()));
    }
}

// Issue 549 (#460 guardrail 6): a medium-risk PR's note also gives its size and `Cargo.lock`
// reasons; it is still never armed (until #550). Low risk keeps its rules.

/// `github(&[3])` with issue 3 labeled `labels` and PR 103's files scripted.
fn github_medium(labels: &str, files: &[&str]) -> FakeGitHub {
    let mut text = issue_text(3);
    text.text = format!("title:\tFix 3\nstate:\tOPEN\nlabels:\t{labels}\n--\nbody\n");
    let files: Vec<PrFile> = files
        .iter()
        .map(|name| PrFile {
            filename: (*name).to_owned(),
            previous_filename: None,
        })
        .collect();
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(text)
        .with_open_issues(vec![open_issue(3, &[READY, "size:S"], "")])
        .with_pr(pr(103, PrState::Open, REVIEWED))
        .with_pr_files(
            103,
            PrFiles {
                changed_files: u32::try_from(files.len()).unwrap(),
                files,
            },
        )
}

const MEDIUM_CLEAN: Review = Review {
    risk: Level::Medium,
    ..LOW_CLEAN
};

#[test]
fn a_medium_pr_of_size_l_changing_cargo_lock_awaits_you_with_both_reasons() {
    let f = fixture();
    let gh = github_medium("ready-for-worker, size:L", &["src/a.rs", "Cargo.lock"]);
    let script = Script::new(&f.base(), MEDIUM_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(
        run.out
            .contains("awaits you: risk medium; its issue #3 is size L; it changes Cargo.lock\n"),
        "{}",
        run.out
    );
}

#[test]
fn a_medium_pr_that_passes_both_checks_still_awaits_you() {
    let f = fixture();
    let gh = github_medium("size:M", &["src/a.rs"]);
    let script = Script::new(&f.base(), MEDIUM_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(run.out.contains("awaits you: risk medium\n"), "{}", run.out);
    assert!(gh.calls().contains(&GhCall::PrFiles(REPO.into(), 103)));
}

#[test]
fn a_low_risk_pr_of_size_l_changing_cargo_lock_is_armed_as_before() {
    let f = fixture();
    let gh = github_medium("size:L", &["Cargo.lock"]);
    let script = Script::new(&f.base(), LOW_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        arms(&gh),
        [GhCall::PrArmAutoMerge(REPO.into(), 103, REVIEWED.into())]
    );
    assert!(!gh.calls().contains(&GhCall::PrFiles(REPO.into(), 103)));
}

#[test]
fn a_draft_pr_is_not_armed() {
    let f = fixture();
    let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
    let script = Script::new(
        &f.base(),
        Review {
            must_fix: 1,
            ..LOW_CLEAN
        },
    );

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(!run.out.contains("auto-merge armed"), "{}", run.out);
}

#[test]
fn a_head_that_moved_after_the_review_is_not_armed_and_the_log_says_why() {
    let f = fixture();
    let gh = github(&[3]).with_pr(pr(103, PrState::Open, MOVED));
    let script = Script::new(&f.base(), LOW_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(
        f.log_text()
            .contains("auto-merge not armed: the PR's head moved after the review"),
        "{}",
        f.log_text()
    );
    assert!(run.out.contains("head moved"), "{}", run.out);
}

#[test]
fn a_pr_with_auto_merge_already_armed_is_left_alone() {
    let f = fixture();
    let gh = github(&[3])
        .with_pr(pr(103, PrState::Open, REVIEWED))
        .with_auto_merge(103);
    let script = Script::new(&f.base(), LOW_CLEAN);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

#[test]
fn a_failed_arming_warns_counts_toward_the_cap_and_the_loop_goes_on() {
    let f = fixture();
    let gh = github(&[3, 4])
        .with_pr(pr(103, PrState::Open, REVIEWED))
        .with_pr(pr(104, PrState::Open, REVIEWED))
        .failing_arm("Pull request is not mergeable");
    let script = Script::new(&f.base(), LOW_CLEAN);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(run.warn.contains("not mergeable"), "{}", run.warn);
    assert!(run.warn.contains("#103"), "{}", run.warn);
    // Both tasks ran: the failure stopped nothing.
    assert!(script.calls().contains(&"finish 4".to_owned()));
    assert_eq!(arms(&gh).len(), 2);
    assert!(
        f.log_text().contains("auto-merge not armed: arming failed"),
        "{}",
        f.log_text()
    );
    // Neither PR has auto-merge, so both await the user.
    use sbxm::github::GitHubBackend;
    assert!(!gh.pr_merge_state(REPO, 103).unwrap().auto_merge);
}

#[test]
fn a_merged_prs_task_is_removed_before_each_new_task() {
    let f = fixture();
    let gh = github(&[3, 4])
        .with_pr(pr(103, PrState::Merged, REVIEWED))
        .with_pr(pr(104, PrState::Open, REVIEWED));
    let script = Script::new(&f.base(), LOW_CLEAN);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    let calls: Vec<String> = script
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("run") || c == "rm-merged")
        .collect();
    assert_eq!(
        calls,
        ["rm-merged", "run 3", "rm-merged", "run 4", "rm-merged"]
    );
}

#[test]
fn a_failed_cleanup_is_a_warning_and_the_loop_goes_on() {
    let f = fixture();
    let gh = github(&[3]).with_pr(pr(103, PrState::Open, REVIEWED));
    let mut script = Script::new(&f.base(), LOW_CLEAN);
    script.rm_fails = true;

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(run.warn.contains("a task folder is locked"), "{}", run.warn);
    assert!(script.calls().contains(&"finish 3".to_owned()));
}

#[test]
fn the_real_cleanup_step_removes_a_finished_task_whose_pr_merged_and_keeps_an_open_one() {
    let f = fixture();
    let gh = github(&[])
        .with_pr(pr(103, PrState::Merged, REVIEWED))
        .with_pr(pr(104, PrState::Open, REVIEWED));
    let script = Script::new(&f.base(), LOW_CLEAN);
    for issue in [3, 4] {
        script.run(issue, &mut Vec::new(), &mut Vec::new()).unwrap();
        script
            .finish(issue, &mut Vec::new(), &mut Vec::new())
            .unwrap();
    }
    let backend = sbxm::backend::FakeBackend::default();
    let host = sbxm::task::gates::FakeHostRunner::default();
    let steps = task_loop::TaskSteps {
        config_dir: &f.env.config_dir(),
        repo_root: f.repo_root.clone(),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
    };

    steps.rm_merged(&mut Vec::new(), &mut Vec::new()).unwrap();

    assert!(!record::task_dir(&f.base(), "issue-3").exists());
    assert!(record::task_dir(&f.base(), "issue-4").exists());
    assert!(
        sbxm::task::merged::archive_dir(&f.base(), "issue-3")
            .join("task.json")
            .is_file()
    );
}
