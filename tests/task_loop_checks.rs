//! Issue 442 (decision 299): `sbxm task loop` reads the workflow runs of the PRs it opened and of
//! `main` on each pass, re-runs a run's failed jobs once, and on a second failure disarms the PR
//! (it awaits the user) or pauses arming on a red `main` until the user runs `task loop
//! --clear-pause`. The phases are scripted; the PRs and the runs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow};
use assert_cmd::Command;
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, Mergeable, PrInfo, PrState, RunStatus, WorkflowRun};
use sbxm::status::looping::{self, Lock, View};
use sbxm::task::loop_pause::{self, Pause, State};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
/// The heads of the PRs the scripted tasks open.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// The head of the loop PR a test starts with.
const OTHER: &str = "2222222222222222222222222222222222222222";
/// A PR's head after a resolve pushed onto it.
const PUSHED: &str = "1111111111111111111111111111111111111111";
/// `main`'s tip.
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// Merge commits of loop PRs.
const MERGE_1: &str = "cccccccccccccccccccccccccccccccccccccccc";
const MERGE_2: &str = "dddddddddddddddddddddddddddddddddddddddd";

/// What a scripted `task run` changes on GitHub after it, given its issue.
type AfterRun<'a> = Box<dyn Fn(u32, &FakeGitHub) + 'a>;

/// Plays `task run`, `task finish` and `task fix --pr N`: each task reaches ready with a clean
/// low-risk review of [`HEAD`], so its PR is armed unless something stops it.
struct Script<'a> {
    base: PathBuf,
    gh: &'a FakeGitHub,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    /// Run after each `task run`, with its issue: GitHub changing meanwhile.
    after_run: AfterRun<'a>,
}

impl<'a> Script<'a> {
    fn new(base: &Path, gh: &'a FakeGitHub) -> Self {
        Self {
            base: base.to_owned(),
            gh,
            calls: RefCell::default(),
            clock: Cell::new(START),
            after_run: Box::new(|_, _| {}),
        }
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

impl Steps for Script<'_> {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        self.clock.set(self.clock.get() + 300);
        write_task(&self.base, Kind::Issue, issue, None, (Level::Low, HEAD));
        (self.after_run)(issue, self.gh);
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

    fn file_findings(
        &self,
        _issue: u32,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Ok(())
    }

    /// A clean low-risk resolve that pushes [`PUSHED`].
    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("fix {pr}"));
        let head = self.gh.pr(REPO, pr)?.head_oid;
        let id = record::fix_id(pr, &head).unwrap();
        let (dir, mut rec) = write_task(&self.base, Kind::Fix, pr, Some(&id), (Level::Low, PUSHED));
        rec.stage = Stage::Finished;
        record::write(&dir, &rec).unwrap();
        self.gh.set_pr_head(pr, PUSHED);
        self.gh.set_mergeable(pr, Mergeable::Mergeable);
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
    Fixture { env, repo_root }
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

    /// A task of issue `issue` a loop finished into PR `100 + issue`, low risk and clean.
    fn loop_pr(&self, issue: u32) {
        let (dir, mut rec) =
            write_task(&self.base(), Kind::Issue, issue, None, (Level::Low, OTHER));
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
        record::write(&dir, &rec).unwrap();
    }

    fn pause(&self) -> State {
        loop_pause::read(&self.base())
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

/// GitHub with open issues `issues` (each finished into PR `100 + issue` on [`HEAD`]), the PRs
/// `prs` on [`OTHER`], and `main` at [`MAIN`].
fn github(issues: &[u32], prs: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN);
    let mut open = Vec::new();
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n)).with_pr(pr(100 + n, HEAD));
        open.push(open_issue(n, &[READY, "size:S"], ""));
    }
    for &n in prs {
        gh = gh.with_pr(pr(n, OTHER));
    }
    gh.with_open_issues(open)
}

struct Ran {
    result: Result<()>,
    out: String,
    warn: String,
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
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn reruns(gh: &FakeGitHub) -> Vec<u64> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::RunRerunFailed(_, id) => Some(id),
            _ => None,
        })
        .collect()
}

fn arms(gh: &FakeGitHub) -> Vec<u32> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::PrArmAutoMerge(_, n, _) => Some(n),
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

fn runs_read(gh: &FakeGitHub) -> Vec<String> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::CommitRuns(_, sha) => Some(sha),
            _ => None,
        })
        .collect()
}

// --- A loop PR's runs ---

#[test]
fn a_first_failure_on_a_loop_pr_is_re_run_once_and_not_disarmed() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[3], &[101])
        .with_auto_merge(101)
        .with_runs(OTHER, vec![run(7, 1, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    // Re-run at the first pass; at the next, its second attempt is still going.
    assert_eq!(reruns(&gh), [7], "{}", ran.out);
    assert!(disarms(&gh).is_empty(), "{}", ran.out);
    assert!(f.pause() == State::None);
}

#[test]
fn a_second_failure_on_a_loop_pr_disarms_it_and_says_so_once() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[3], &[101])
        .with_auto_merge(101)
        .with_runs(OTHER, vec![run(7, 2, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(reruns(&gh).is_empty());
    assert_eq!(disarms(&gh), [101], "{}", ran.out);
    let said: Vec<&str> = ran
        .out
        .lines()
        .filter(|l| l.contains("PR #101") && l.contains("again after a re-run"))
        .collect();
    assert_eq!(said.len(), 1, "{}", ran.out);
    assert!(said[0].contains("awaits you"), "{}", said[0]);
    let log = f.log_text();
    let rows: Vec<&str> = log.lines().filter(|l| l.contains("#101")).collect();
    assert_eq!(rows.len(), 1, "{log}");
    // Not a red main: nothing paused, and the next task's PR is armed.
    assert!(f.pause() == State::None);
    assert_eq!(arms(&gh), [103]);
}

#[test]
fn a_second_failure_on_an_unarmed_loop_pr_is_said_without_a_disarm() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[], &[101]).with_runs(OTHER, vec![run(7, 3, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(disarms(&gh).is_empty());
    assert!(ran.out.contains("PR #101 awaits you"), "{}", ran.out);
}

#[test]
fn a_re_run_that_passed_ends_it() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[3], &[101])
        .with_auto_merge(101)
        .with_runs(OTHER, vec![run(7, 2, RunStatus::Passed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(reruns(&gh).is_empty());
    assert!(disarms(&gh).is_empty());
    assert!(!ran.out.contains("PR #101"), "{}", ran.out);
}

#[test]
fn runs_still_going_or_queued_do_nothing() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[3], &[101])
        .with_auto_merge(101)
        .with_runs(
            OTHER,
            vec![run(7, 1, RunStatus::Going), run(8, 2, RunStatus::Going)],
        )
        .with_runs(MAIN, vec![run(9, 2, RunStatus::Going)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(reruns(&gh).is_empty());
    assert!(disarms(&gh).is_empty());
    assert!(f.pause() == State::None);
    assert_eq!(arms(&gh), [103]);
}

#[test]
fn a_pr_the_loop_did_not_open_has_its_runs_unread() {
    let f = fixture();
    // Finished by a command run by hand: no loop mark.
    let (dir, mut rec) = write_task(&f.base(), Kind::Issue, 1, None, (Level::Low, OTHER));
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/101"));
    record::write(&dir, &rec).unwrap();
    let gh = github(&[], &[101])
        .with_auto_merge(101)
        .with_runs(OTHER, vec![run(7, 2, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &script, &gh).result.unwrap();

    assert!(!runs_read(&gh).contains(&OTHER.to_owned()));
    assert!(disarms(&gh).is_empty());
}

#[test]
fn a_red_draft_loop_pr_is_re_run_once_and_never_armed() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[], &[101])
        .with_draft(101)
        .with_runs(OTHER, vec![run(7, 1, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &script, &gh).result.unwrap();

    assert_eq!(reruns(&gh), [7]);
    assert!(arms(&gh).is_empty());
}

// --- `main` ---

#[test]
fn a_first_failure_on_mains_tip_is_re_run_and_does_not_pause() {
    let f = fixture();
    let gh = github(&[3], &[]).with_runs(MAIN, vec![run(9, 1, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(reruns(&gh), [9]);
    assert!(f.pause() == State::None);
    assert_eq!(arms(&gh), [103]);
}

#[test]
fn a_second_failure_on_mains_tip_after_the_users_merge_pauses_and_disarms_the_loop_prs() {
    let f = fixture();
    f.loop_pr(1);
    let gh = github(&[3], &[101])
        .with_auto_merge(101)
        .with_runs(MAIN, vec![run(9, 2, RunStatus::Failed)]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    let State::Set(pause) = f.pause() else {
        panic!("not paused: {}", ran.out);
    };
    assert_eq!(pause.commit, MAIN);
    assert_eq!(pause.run_id, 9);
    assert_eq!(pause.run_url, "https://github.com/o/r/actions/runs/9");
    assert_eq!(pause.merged_by, "alice");
    let line = ran
        .out
        .lines()
        .find(|l| l.contains(MAIN))
        .unwrap_or_else(|| panic!("{}", ran.out));
    assert!(line.contains("alice"), "{line}");
    assert!(line.contains("--clear-pause"), "{line}");
    // The armed loop PR is disarmed, and the next task's PR is not armed.
    assert_eq!(disarms(&gh), [101], "{}", ran.out);
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    // Said once, though the tip is read again at the next pass.
    assert_eq!(ran.out.matches(MAIN).count(), 1, "{}", ran.out);
}

#[test]
fn two_loop_armed_prs_merged_since_the_last_pass_one_red_pauses_naming_that_one() {
    let f = fixture();
    let gh = github(&[1, 2, 3], &[])
        .with_runs(MERGE_1, vec![run(11, 2, RunStatus::Failed)])
        .with_runs(MERGE_2, vec![run(12, 1, RunStatus::Passed)]);
    let mut script = Script::new(&f.base(), &gh);
    // 101 and 102 were armed by the loop when their tasks finished; both merge meanwhile.
    script.after_run = Box::new(|issue, gh| {
        if issue == 3 {
            gh.set_merged(101, MERGE_1);
            gh.set_merged(102, MERGE_2);
        }
    });

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert_eq!(arms(&gh), [101, 102, 103], "{}", ran.out);
    let read = runs_read(&gh);
    assert!(read.contains(&MERGE_1.to_owned()), "{read:?}");
    assert!(read.contains(&MERGE_2.to_owned()), "{read:?}");
    let State::Set(pause) = f.pause() else {
        panic!("not paused: {}", ran.out);
    };
    assert_eq!(pause.commit, MERGE_1);
    assert!(pause.merged_by.contains("PR #101"), "{}", pause.merged_by);
    assert_eq!(disarms(&gh), [103], "{}", ran.out);
}

#[test]
fn a_merge_commit_whose_runs_are_still_going_is_read_again_at_the_next_pass() {
    let f = fixture();
    let gh = github(&[1, 2, 3], &[]);
    let mut script = Script::new(&f.base(), &gh);
    script.after_run = Box::new(|issue, gh| match issue {
        2 => {
            gh.set_merged(101, MERGE_1);
            gh.set_runs(MERGE_1, vec![run(11, 1, RunStatus::Going)]);
        }
        3 => gh.set_runs(MERGE_1, vec![run(11, 2, RunStatus::Failed)]),
        _ => {}
    });

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    let State::Set(pause) = f.pause() else {
        panic!("not paused: {}", ran.out);
    };
    assert_eq!(pause.commit, MERGE_1);
}

#[test]
fn while_paused_a_clean_low_risk_task_is_not_armed() {
    let f = fixture();
    f.write_pause();
    let gh = github(&[3], &[]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .lines()
            .any(|l| l.contains("PR #103") && l.contains("paused")),
        "{}",
        ran.out
    );
    // The loop never lifts it itself.
    assert!(matches!(f.pause(), State::Set(_)));
}

#[test]
fn while_paused_a_resolve_still_runs_but_is_not_armed_again() {
    let f = fixture();
    f.write_pause();
    f.loop_pr(1);
    let gh = github(&[], &[101])
        .with_auto_merge(101)
        .with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(script.calls.borrow().contains(&"fix 101".to_owned()));
    assert!(arms(&gh).is_empty(), "{}", ran.out);
}

#[test]
fn an_unreadable_pause_file_counts_as_paused_with_a_warning_naming_it() {
    let f = fixture();
    let path = loop_pause::path(&f.base());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ not json").unwrap();
    let gh = github(&[3], &[]);
    let script = Script::new(&f.base(), &gh);

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.warn.contains(&path.display().to_string()),
        "{}",
        ran.warn
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn a_pause_the_user_cleared_is_not_set_again_for_the_same_run() {
    let f = fixture();
    let gh = github(&[3], &[]).with_runs(MAIN, vec![run(9, 2, RunStatus::Failed)]);
    let mut script = Script::new(&f.base(), &gh);
    let base = f.base();
    // The user clears it while task 3 runs.
    script.after_run = Box::new(move |_, _| {
        loop_pause::clear(&base, &mut Vec::new()).unwrap();
    });

    let ran = run_loop(&f, &script, &gh);

    ran.result.unwrap();
    assert!(f.pause() == State::None, "{}", ran.out);
    assert_eq!(arms(&gh), [103], "{}", ran.out);
}

// --- `--clear-pause` ---

fn sbxm(f: &Fixture) -> Command {
    let mut cmd = Command::cargo_bin("sbxm").unwrap();
    cmd.env("SBXM_CONFIG_DIR", f.env.config_dir())
        .current_dir(&f.repo_root);
    cmd
}

#[test]
fn clear_pause_with_no_pause_says_there_was_none() {
    let f = fixture();

    let output = sbxm(&f)
        .args(["task", "loop", "--clear-pause"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("no pause"), "{stdout}");
}

#[test]
fn clear_pause_removes_it_and_says_what_it_removed() {
    let f = fixture();
    f.write_pause();

    let output = sbxm(&f)
        .args(["task", "loop", "--clear-pause"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("cleared the pause"), "{stdout}");
    assert!(stdout.contains(MAIN), "{stdout}");
    assert!(stdout.contains("alice"), "{stdout}");
    assert!(f.pause() == State::None);
}

#[test]
fn clear_pause_removes_a_damaged_pause_file() {
    let f = fixture();
    let path = loop_pause::path(&f.base());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "garbage").unwrap();
    let mut out = Vec::new();

    loop_pause::clear(&f.base(), &mut out).unwrap();

    assert!(!path.exists());
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains(&path.display().to_string()), "{out}");
}

#[test]
fn clear_pause_refuses_a_folder_in_the_pause_files_place() {
    let f = fixture();
    let path = loop_pause::path(&f.base());
    fs::create_dir_all(&path).unwrap();

    let e = loop_pause::clear(&f.base(), &mut Vec::new()).unwrap_err();

    assert!(path.is_dir());
    assert!(format!("{e:#}").contains("not a plain file"), "{e:#}");
}

#[test]
fn clear_pause_does_not_go_with_issues_or_a_cap() {
    let f = fixture();
    sbxm(&f)
        .args(["task", "loop", "--clear-pause", "--issue", "3"])
        .assert()
        .failure();
}

// --- `sbxm status` ---

#[test]
fn the_status_loop_section_shows_the_pause() {
    let f = fixture();
    f.write_pause();
    let view = View {
        lock: Lock::Absent,
        work: None,
        snapshot: None,
        pause: loop_pause::read(&f.base()),
    };

    let text = looping::render(&view, None);

    let line = text
        .lines()
        .find(|l| l.contains("paused"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(line.contains(MAIN), "{line}");
    assert!(line.contains("alice"), "{line}");
    assert!(line.contains("--clear-pause"), "{line}");
}

#[test]
fn the_status_loop_section_shows_an_unreadable_pause() {
    let view = View {
        lock: Lock::Absent,
        work: None,
        snapshot: None,
        pause: State::Unreadable("cannot parse /b/.sbxm/loop-pause.json".into()),
    };

    let text = looping::render(&view, None);

    assert!(text.contains("/b/.sbxm/loop-pause.json"), "{text}");
}

#[test]
fn the_status_loop_section_has_no_pause_line_without_a_pause() {
    let view = View {
        lock: Lock::Absent,
        work: None,
        snapshot: None,
        pause: State::None,
    };

    assert!(!looping::render(&view, None).contains("paused"));
}

// --- `gh` ---

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

fn gh_answering(output: &str) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(vec![output.to_owned()]),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

/// Written by hand in the shape of GitHub's REST answer (no GitHub access where issue 442 was
/// built), cut to the fields read.
const RUNS: &str = r#"{"total_count":4,"workflow_runs":[
 {"id":1,"name":"ci","run_attempt":1,"status":"in_progress","conclusion":null,"html_url":"u1","actor":{"login":"alice"}},
 {"id":2,"name":"ci","run_attempt":2,"status":"completed","conclusion":"failure","html_url":"u2","actor":{"login":"bob"}},
 {"id":3,"name":"CodeQL","run_attempt":1,"status":"completed","conclusion":"success","html_url":"u3","actor":null},
 {"id":4,"name":"ci","run_attempt":1,"status":"completed","conclusion":"timed_out","html_url":"u4"}
]}"#;

#[test]
fn gh_reads_a_commits_runs_with_their_attempts() {
    let (gh, script) = gh_answering(RUNS);

    let runs = gh.commit_runs(REPO, MAIN).unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0],
        [
            "api".to_owned(),
            format!("repos/{REPO}/actions/runs?head_sha={MAIN}&per_page=100")
        ]
    );
    let got: Vec<(u64, u32, RunStatus, &str)> = runs
        .iter()
        .map(|r| (r.id, r.attempt, r.status, r.actor.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (1, 1, RunStatus::Going, "alice"),
            (2, 2, RunStatus::Failed, "bob"),
            (3, 1, RunStatus::Passed, ""),
            (4, 1, RunStatus::Failed, ""),
        ]
    );
    assert_eq!(runs[2].workflow, "CodeQL");
    assert_eq!(runs[1].url, "u2");
}

#[test]
fn gh_refuses_a_short_commit_for_runs_without_calling_gh() {
    let (gh, script) = gh_answering(RUNS);

    assert!(gh.commit_runs(REPO, "abc").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn gh_re_runs_only_the_failed_jobs() {
    let (gh, script) = gh_answering("");

    gh.run_rerun_failed(REPO, 42).unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0],
        ["run", "rerun", "42", "--failed", "--repo", REPO]
    );
}

#[test]
fn gh_reads_a_merged_prs_merge_commit_and_none_for_an_open_one() {
    let merged = format!(r#"{{"state":"MERGED","mergeCommit":{{"oid":"{MERGE_1}"}}}}"#);
    let (gh, script) = gh_answering(&merged);
    assert_eq!(
        gh.pr_merge_commit(REPO, 5).unwrap().as_deref(),
        Some(MERGE_1)
    );
    assert_eq!(
        script.calls.lock().unwrap()[0],
        [
            "pr",
            "view",
            "5",
            "--repo",
            REPO,
            "--json",
            "state,mergeCommit"
        ]
    );

    let (gh, _) = gh_answering(r#"{"state":"OPEN","mergeCommit":null}"#);
    assert_eq!(gh.pr_merge_commit(REPO, 5).unwrap(), None);
}
