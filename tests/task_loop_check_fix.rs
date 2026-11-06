//! Issue 448 (decision 300): on a second failure on a loop PR, before it is left to the user,
//! `sbxm task loop` runs one log-fix, `task fix --pr N --check-run <id>`, ever per PR, counted
//! from the task records (a fix task of the PR whose folder holds `check-log.md`). A clean one is
//! armed again only on decision 298's rules and never while the loop is paused. The phases are
//! scripted; the PRs and the runs are `FakeGitHub`'s.

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
use sbxm::github::{GitHubBackend, PrInfo, PrState, RunStatus, WorkflowRun};
use sbxm::task::loop_pause::{self, Pause};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
/// The head of the loop PR a test starts with.
const OTHER: &str = "2222222222222222222222222222222222222222";
/// The PR's head after a log-fix pushed onto it.
const PUSHED: &str = "1111111111111111111111111111111111111111";
/// `main`'s tip.
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// How a scripted log-fix ends.
#[derive(Clone, Copy)]
enum Ends {
    /// Pushed [`PUSHED`], its review clean at that risk and covering it.
    Clean(Level),
    /// Pushed, its review left one must-fix.
    MustFix,
    /// Pushed with a clean review, but its result hands a question back.
    HandsBack,
    /// Failed after its task was created (the folder has `check-log.md`).
    Fails,
    /// Refused before anything was created.
    Refused,
}

struct Script<'a> {
    base: PathBuf,
    gh: &'a FakeGitHub,
    ends: Ends,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    /// Run after a `task run`: GitHub changing meanwhile.
    after_run: Box<dyn Fn(&FakeGitHub) + 'a>,
}

/// The head of the PR a scripted `task run` opens.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

impl<'a> Script<'a> {
    fn new(base: &Path, gh: &'a FakeGitHub, ends: Ends) -> Self {
        Self {
            base: base.to_owned(),
            gh,
            ends,
            calls: RefCell::default(),
            clock: Cell::new(START),
            after_run: Box::new(|_| {}),
        }
    }

    fn log_fixes(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| c.starts_with("log-fix"))
            .cloned()
            .collect()
    }
}

fn write_task(
    base: &Path,
    kind: Kind,
    number: u32,
    id: Option<&str>,
    review: (Level, &str),
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
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.review = Some(ReviewResult {
        round: 1,
        commit: Some(review.1.to_owned()),
        must_fix: 0,
        risk: review.0,
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
    (dir, rec)
}

/// A finished fix task of PR `pr` at `head`; a log-fix when `log` is set.
fn write_fix(base: &Path, pr: u32, head: &str, log: bool) -> PathBuf {
    let id = record::fix_id(pr, head).unwrap();
    let (dir, mut rec) = write_task(base, Kind::Fix, pr, Some(&id), (Level::Low, PUSHED));
    rec.stage = Stage::Finished;
    record::write(&dir, &rec).unwrap();
    if log {
        fs::write(
            dir.join("check-log.md"),
            "Failed run 7 (ci), job test: ...\n",
        )
        .unwrap();
    }
    dir
}

impl Steps for Script<'_> {
    /// A task that reaches ready with a clean low-risk review of [`HEAD`].
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.clock.set(self.clock.get() + 300);
        write_task(&self.base, Kind::Issue, issue, None, (Level::Low, HEAD));
        (self.after_run)(self.gh);
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec).unwrap();
        Ok(())
    }

    fn file_findings(
        &self,
        _issue: u32,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
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
        self.calls.borrow_mut().push(format!("log-fix {pr} {run}"));
        if matches!(self.ends, Ends::Refused) {
            return Err(anyhow!("task fix-{pr} exists"));
        }
        let head = self.gh.pr(REPO, pr)?.head_oid;
        let dir = write_fix(&self.base, pr, &head, true);
        let path = dir.join("task.json");
        let mut rec = record::read(&path).unwrap();
        match self.ends {
            Ends::Clean(risk) => rec.review.as_mut().unwrap().risk = risk,
            Ends::MustFix => rec.review.as_mut().unwrap().must_fix = 1,
            Ends::HandsBack => fs::write(
                dir.join("result.md"),
                "The check passes.\n\n## Choices made\n\nnone\n\n## Needs a decision\n\n\
                 - Should the flaky test be skipped? I would keep it.\n",
            )
            .unwrap(),
            Ends::Fails => return Err(anyhow!("gates failed for fix-{pr}")),
            Ends::Refused => unreachable!(),
        }
        record::write(&dir, &rec).unwrap();
        self.gh.set_pr_head(pr, PUSHED);
        Ok(())
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
    let f = Fixture { env, repo_root };
    // The task of issue 1, which a loop finished into PR 101, low risk and clean.
    let (dir, mut rec) = write_task(&f.base(), Kind::Issue, 1, None, (Level::Low, OTHER));
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/101"));
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    record::write(&dir, &rec).unwrap();
    f
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn log_text(&self) -> String {
        fs::read_to_string(self.repo_root.join("sdlc/batches/2026-11-02-loop.md"))
            .unwrap_or_default()
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
        }
    }

    fn write_pause(&self) {
        loop_pause::write(
            &self.base(),
            &Pause {
                commit: MAIN.into(),
                run_id: 9,
                run_url: "https://github.com/o/r/actions/runs/9".into(),
                workflow: "ci".into(),
                merged_by: "alice".into(),
                at: "2026-11-01T10:00:00Z".into(),
            },
        )
        .unwrap();
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

fn run(id: u64, attempt: u32, status: RunStatus) -> WorkflowRun {
    WorkflowRun {
        id,
        workflow: "ci".into(),
        attempt,
        status,
        url: format!("https://github.com/o/r/actions/runs/{id}"),
        actor: "alice".into(),
    }
}

/// PR 101 on [`OTHER`] with auto-merge armed, whose run 7 failed again after a re-run; no open
/// issues, so the loop makes one pass and stops on its empty queue.
fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN)
        .with_pr(pr(101, OTHER))
        .with_auto_merge(101)
        .with_runs(OTHER, vec![run(7, 2, RunStatus::Failed)])
}

struct Ran {
    result: Result<()>,
    out: String,
}

fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub) -> Ran {
    let sleeps = Cell::new(0usize);
    let now = || script.clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        script.clock.set(script.clock.get() + d.as_secs());
    };
    let stop = || sleeps.get() > 0;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        &f.opts(),
        script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
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

fn disarms(gh: &FakeGitHub) -> Vec<u32> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::PrDisarmAutoMerge(_, n) => Some(n),
            _ => None,
        })
        .collect()
}

/// The lines saying PR 101 awaits the user.
fn awaits(out: &str) -> Vec<&str> {
    out.lines()
        .filter(|l| l.contains("PR #101 awaits you"))
        .collect()
}

#[test]
fn a_second_failure_with_no_log_fix_yet_runs_one_and_a_clean_one_is_armed_again() {
    let f = fixture();
    let gh = github();
    let script = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(script.log_fixes(), ["log-fix 101 7"], "{}", ran.out);
    // Disarmed before the push, then armed again on the head the log-fix's review covered.
    assert_eq!(disarms(&gh), [101], "{}", ran.out);
    assert_eq!(arms(&gh), [(101, PUSHED.to_owned())], "{}", ran.out);
    assert!(
        ran.out.contains("--check-run 7`") && ran.out.contains("PR #101: fixed from its log"),
        "{}",
        ran.out
    );
    assert!(awaits(&ran.out).is_empty(), "{}", ran.out);
    let log = f.log_text();
    let rows: Vec<&str> = log.lines().filter(|l| l.contains("#101")).collect();
    assert_eq!(rows.len(), 1, "{log}");
    assert!(rows[0].contains("log-fix"), "{}", rows[0]);
}

#[test]
fn a_second_failure_after_a_log_fix_disarms_and_leaves_it_to_the_user_as_before() {
    let f = fixture();
    write_fix(
        &f.base(),
        101,
        "3333333333333333333333333333333333333333",
        true,
    );
    let gh = github();
    let script = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(script.log_fixes().is_empty(), "{}", ran.out);
    assert_eq!(disarms(&gh), [101], "{}", ran.out);
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    let said = awaits(&ran.out);
    assert_eq!(said.len(), 1, "{}", ran.out);
    assert!(
        said[0].contains("failed again after a re-run"),
        "{}",
        said[0]
    );
}

#[test]
fn a_resolve_or_a_log_fix_removed_by_hand_is_no_log_fix() {
    let f = fixture();
    // A resolve's fix task has no check-log.md; a log-fix's folder removed by hand is gone.
    write_fix(
        &f.base(),
        101,
        "3333333333333333333333333333333333333333",
        false,
    );
    let removed = write_fix(
        &f.base(),
        101,
        "4444444444444444444444444444444444444444",
        true,
    );
    fs::remove_dir_all(&removed).unwrap();
    let gh = github();
    let script = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(script.log_fixes(), ["log-fix 101 7"], "{}", ran.out);
}

#[test]
fn a_loop_restart_after_a_log_fix_does_not_try_again() {
    let f = fixture();
    let gh = github();
    let first = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));
    run_loop(&f, &first, &gh).result.unwrap();
    assert_eq!(first.log_fixes().len(), 1);
    // The fixed head fails twice too.
    gh.set_runs(PUSHED, vec![run(8, 2, RunStatus::Failed)]);

    let again = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));
    let ran = run_loop(&f, &again, &gh);

    ran.result.unwrap();
    assert!(again.log_fixes().is_empty(), "{}", ran.out);
    assert_eq!(disarms(&gh), [101, 101], "{}", ran.out);
    let said = awaits(&ran.out);
    assert_eq!(said.len(), 1, "{}", ran.out);
    assert!(said[0].contains("runs/8"), "{}", said[0]);
}

#[test]
fn a_log_fix_while_paused_runs_and_is_not_armed_again() {
    let f = fixture();
    f.write_pause();
    let gh = github();
    let script = Script::new(&f.base(), &gh, Ends::Clean(Level::Low));

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(script.log_fixes(), ["log-fix 101 7"], "{}", ran.out);
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .lines()
            .any(|l| l.contains("PR #101 awaits you") && l.contains("paused")),
        "{}",
        ran.out
    );
}

/// A log-fix that ended as `ends`: the PR is not armed again and awaits the user, said once.
fn not_clean(ends: Ends, why: &str) {
    let f = fixture();
    let gh = github();
    let script = Script::new(&f.base(), &gh, ends);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(script.log_fixes(), ["log-fix 101 7"], "{}", ran.out);
    assert_eq!(disarms(&gh), [101], "{}", ran.out);
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    let said = awaits(&ran.out);
    assert_eq!(said.len(), 1, "{}", ran.out);
    assert!(said[0].contains(why), "{}", said[0]);
}

#[test]
fn a_log_fix_that_left_must_fix_leaves_the_pr_to_the_user() {
    not_clean(Ends::MustFix, "left 1 must-fix");
}

#[test]
fn a_log_fix_that_hands_a_question_back_leaves_the_pr_to_the_user() {
    not_clean(Ends::HandsBack, "handed a question back");
}

#[test]
fn a_failed_log_fix_leaves_the_pr_to_the_user() {
    not_clean(Ends::Fails, "gates failed");
}

#[test]
fn a_clean_log_fix_of_medium_risk_is_not_armed_again() {
    not_clean(Ends::Clean(Level::Medium), "risk medium");
}

#[test]
fn a_log_fix_refused_before_its_task_is_not_tried_again_in_the_same_run() {
    let f = fixture();
    // Issue 3 gives the loop a second pass, after which the same head has a newer second
    // failure.
    let gh = github()
        .with_issue_text(issue_text(3))
        .with_pr(pr(103, HEAD))
        .with_open_issues(vec![open_issue(3, &["ready-for-worker", "size:S"], "")]);
    let mut script = Script::new(&f.base(), &gh, Ends::Refused);
    script.after_run = Box::new(|gh| {
        gh.set_runs(
            OTHER,
            vec![run(7, 2, RunStatus::Failed), run(8, 2, RunStatus::Failed)],
        );
    });

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(script.log_fixes(), ["log-fix 101 7"], "{}", ran.out);
    let said = awaits(&ran.out);
    assert_eq!(said.len(), 2, "{}", ran.out);
    assert!(said[0].contains("its log-fix failed"), "{}", said[0]);
    assert!(said[1].contains("runs/8"), "{}", said[1]);
}
