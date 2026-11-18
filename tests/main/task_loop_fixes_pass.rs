//! Issue 707: the resolve re-arm and the step that re-checks waiting PRs judge the fix tasks
//! already pushed onto a PR (`fix-N-<head>`) by one rule, read from their records: a folder or a
//! record that can't be read doesn't pass, and a log-fix (its folder holds `check-log.md`) passes
//! only finished and clean. So a restarted loop, with nothing in memory, doesn't arm a PR an
//! earlier loop left to the user. The phases are scripted; the PRs are `FakeGitHub`'s.

use crate::common;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::Probe;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{CommitStatus, GitHubBackend, Mergeable, PrInfo, PrState, StatusState};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
/// The loop PR's head, the commit its own task's review covered.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// The head an earlier fix task of the PR started from.
const EARLIER: &str = "3333333333333333333333333333333333333333";
/// The PR's head after a scripted resolve pushed onto it.
const RESOLVED: &str = "5555555555555555555555555555555555555555";
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The loop PR of the tests, issue 1's.
const PR: u32 = 101;

/// No issue runs: the queue is empty. A resolve is clean, low risk, and pushes [`RESOLVED`].
struct Script<'a> {
    base: PathBuf,
    gh: &'a FakeGitHub,
}

impl Steps for Script<'_> {
    fn run(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of {issue}"))
    }
    fn resume(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }
    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of {issue}"))
    }
    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }
    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        let head = self.gh.pr(REPO, pr)?.head_oid;
        let id = record::fix_id(pr, &head).unwrap();
        write_task(
            &self.base,
            Kind::Fix,
            pr,
            Some(&id),
            Stage::Finished,
            RESOLVED,
        );
        self.gh.set_pr_head(pr, RESOLVED);
        self.gh.set_mergeable(pr, Mergeable::Mergeable);
        Ok(())
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

/// A task record at `stage` whose clean low-risk review covered `reviewed`.
fn write_task(
    base: &Path,
    kind: Kind,
    number: u32,
    id: Option<&str>,
    stage: Stage,
    reviewed: &str,
) -> (PathBuf, Record) {
    let id_text = id.map_or_else(|| format!("issue-{number}"), str::to_owned);
    let dir = record::task_dir(base, &id_text);
    let mut rec = Record::new(
        &NewTask {
            kind,
            number,
            repo: REPO,
            title: &format!("Fix {number}"),
            base: "main",
            branch: &format!("sbxm/{id_text}"),
            config_hash: "h",
            id,
        },
        START,
        Process::new(1, 0),
    );
    rec.stage = stage;
    rec.status = Status::Ok;
    rec.review = Some(ReviewResult {
        round: 1,
        commit: Some(reviewed.to_owned()),
        must_fix: 0,
        risk: Level::Low,
        not_run: Some(Vec::new()),
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
    (dir, rec)
}

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

/// Issue 1's task, which a loop finished into PR 101, low risk and clean on [`HEAD`]; its issue
/// text is `body`.
fn fixture(body: &str) -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let f = Fixture { env, repo_root };
    let (dir, mut rec) = write_task(&f.base(), Kind::Issue, 1, None, Stage::Finished, HEAD);
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{PR}"));
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    record::write(&dir, &rec).unwrap();
    fs::write(
        dir.join("issue.md"),
        format!("title:\tFix 1\nstate:\tOPEN\nlabels:\tsize:S\n--\n{body}"),
    )
    .unwrap();
    f
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn log_text(&self) -> String {
        let dir = self.base().join(".sbxm/loop-logs");
        fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|e| fs::read_to_string(e.ok()?.path()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An earlier log-fix of PR 101, from [`EARLIER`], that stopped at `stage`.
    fn log_fix_at(&self, stage: Stage) {
        let id = record::fix_id(PR, EARLIER).unwrap();
        let (dir, _) = write_task(&self.base(), Kind::Fix, PR, Some(&id), stage, HEAD);
        fs::write(dir.join("check-log.md"), "Failed run 7 (ci): ...\n").unwrap();
    }

    /// An earlier fix task of PR 101 whose `task.json` can't be read.
    fn unreadable_fix(&self) {
        let id = record::fix_id(PR, EARLIER).unwrap();
        let (dir, _) = write_task(
            &self.base(),
            Kind::Fix,
            PR,
            Some(&id),
            Stage::Finished,
            HEAD,
        );
        fs::write(dir.join("task.json"), "{ not json").unwrap();
    }
}

fn pr(number: u32, head: &str) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// PR 101 open and unarmed on [`HEAD`]; no open issues.
fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN)
        .with_pr(pr(PR, HEAD))
}

fn real_check_passed(gh: &FakeGitHub, sha: &str) {
    gh.commit_status(
        REPO,
        sha,
        &CommitStatus {
            state: StatusState::Success,
            context: "sbxm/real-check".into(),
            description: "ok".into(),
        },
    )
    .unwrap();
}

struct Ran {
    out: String,
}

/// One pass of a fresh loop: nothing in memory from an earlier one.
fn run_loop(f: &Fixture, gh: &FakeGitHub) -> Ran {
    let clock = Cell::new(START);
    let sleeps = Cell::new(0usize);
    let now = || clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        clock.set(clock.get() + d.as_secs());
    };
    let stop = || sleeps.get() > 0;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let opts = Options {
        repo_root: f.repo_root.clone(),
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let script = Script { base: f.base(), gh };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run(
        &f.env.config_dir(),
        &opts,
        &script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    )
    .unwrap();
    Ran {
        out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
    }
}

fn arms(gh: &FakeGitHub) -> Vec<(u32, String)> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::PrArmAutoMerge(_, n, head) => Some((n, head)),
            _ => None,
        })
        .collect()
}

// --- The waiting-PR recheck ---

const REAL_CHECK: &str = "**Real check:** run it\n";

/// PR 101, held for its real check, which has passed on [`HEAD`]; `before` adds the earlier fix
/// tasks. A fresh loop's recheck: what it armed.
fn rechecked(before: &dyn Fn(&Fixture)) -> (Fixture, Ran, Vec<(u32, String)>) {
    let f = fixture(REAL_CHECK);
    before(&f);
    let gh = github();
    real_check_passed(&gh, HEAD);
    let ran = run_loop(&f, &gh);
    let arms = arms(&gh);
    (f, ran, arms)
}

#[test]
fn the_recheck_arms_with_no_earlier_fix() {
    let (_, ran, arms) = rechecked(&|_| {});
    assert_eq!(arms, [(PR, HEAD.to_owned())], "{}", ran.out);
}

#[test]
fn the_recheck_arms_after_a_clean_log_fix() {
    let (_, ran, arms) = rechecked(&|f| f.log_fix_at(Stage::Finished));
    assert_eq!(arms, [(PR, HEAD.to_owned())], "{}", ran.out);
}

fn not_armed_after_a_restart(before: &dyn Fn(&Fixture)) {
    let (f, ran, arms) = rechecked(before);
    assert!(arms.is_empty(), "{}", ran.out);
    let why = "auto-merge not armed: an earlier fix task of it didn't pass";
    assert!(f.log_text().contains(why), "{}", f.log_text());
    assert!(
        ran.out
            .contains("PR #101 (Fix 1) awaits you: an earlier fix task of it didn't pass"),
        "{}",
        ran.out
    );
}

#[test]
fn the_recheck_does_not_arm_after_a_log_fix_stopped_at_working() {
    not_armed_after_a_restart(&|f| f.log_fix_at(Stage::Working));
}

#[test]
fn the_recheck_does_not_arm_after_a_log_fix_stopped_at_ready() {
    // Its gates failed: the task never got to `finished`.
    not_armed_after_a_restart(&|f| f.log_fix_at(Stage::Ready));
}

#[test]
fn the_recheck_does_not_arm_with_an_unreadable_fix_record() {
    not_armed_after_a_restart(&|f| f.unreadable_fix());
}

// --- The resolve re-arm ---

/// PR 101 conflicting with main, resolved cleanly by a fresh loop; `before` adds the earlier fix
/// tasks. Whether the resolve armed it.
fn rearmed_by_a_resolve(before: &dyn Fn(&Fixture)) -> bool {
    let f = fixture("Do it.\n");
    before(&f);
    let gh = github();
    gh.set_mergeable(PR, Mergeable::Conflicting);
    let ran = run_loop(&f, &gh);
    assert!(ran.out.contains("resolved"), "{}", ran.out);
    arms(&gh).contains(&(PR, RESOLVED.to_owned()))
}

#[test]
fn a_resolve_rearms_with_no_earlier_fix() {
    assert!(rearmed_by_a_resolve(&|_| {}));
}

#[test]
fn a_resolve_does_not_rearm_with_an_unreadable_fix_record() {
    assert!(!rearmed_by_a_resolve(&|f| f.unreadable_fix()));
}

#[test]
fn a_resolve_does_not_rearm_after_a_log_fix_stopped_at_working() {
    assert!(!rearmed_by_a_resolve(&|f| f.log_fix_at(Stage::Working)));
}
