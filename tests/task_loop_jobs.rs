//! Issue 525 (decision 309): `sbxm task loop` drains a queue folder of job files,
//! `<base>/.sbxm/queue/*.json`, before each new issue: `review`, `fix`, `run` and `rm-merged`,
//! each ending with one result file in `queue/done/`. A job outside the four shapes is refused
//! and runs nothing; a job left in `queue/running/` by a stopped loop is reported, never re-run.
//! The phases are scripted; the PRs and issues are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::jobs::{self, Job};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;
use serde_json::Value;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// Plays the loop's phases, recording each call; the phases named in `fail` fail.
struct Script {
    base: PathBuf,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    fail: Vec<String>,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            calls: RefCell::default(),
            clock: Cell::new(START),
            fail: Vec::new(),
        }
    }

    fn failing(mut self, call: &str) -> Self {
        self.fail.push(call.to_owned());
        self
    }

    /// Every call but the loop's own `rm --merged` before each step.
    fn calls(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| *c != "rm-merged (loop)")
            .cloned()
            .collect()
    }

    fn call(&self, call: String) -> Result<()> {
        self.clock.set(self.clock.get() + 60);
        let failing = self.fail.contains(&call);
        self.calls.borrow_mut().push(call.clone());
        if failing {
            bail!("{call} failed");
        }
        Ok(())
    }
}

/// A task record of `kind` in `base`, ready, with a clean low-risk review.
fn write_task(base: &Path, kind: Kind, number: u32, id: Option<&str>, repo: &str) -> PathBuf {
    let id_text = id.map_or_else(|| format!("{}-{number}", prefix(kind)), str::to_owned);
    let dir = record::task_dir(base, &id_text);
    let mut rec = Record::new(
        &NewTask {
            kind,
            number,
            repo,
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
        commit: Some(HEAD.to_owned()),
        must_fix: 0,
        risk: Level::Low,
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
    dir
}

fn prefix(kind: Kind) -> &'static str {
    match kind {
        Kind::Issue => "issue",
        Kind::Pr => "pr",
        Kind::Spec => "spec",
        Kind::Fix => "fix",
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("run {issue}"))?;
        write_task(&self.base, Kind::Issue, issue, None, REPO);
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("finish {issue}"))?;
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
        self.call(format!("fix {pr}"))
    }

    fn fix_check(
        &self,
        pr: u32,
        run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        self.call(format!("fix {pr} --check-run {run}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        // A job's own runs while its file is in `running/`; the loop's before each step doesn't.
        let running = jobs::running_dir(&self.base);
        let job = fs::read_dir(&running).is_ok_and(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| fs::read_to_string(e.path()).is_ok_and(|text| text.contains("rm-merged")))
        });
        if !job {
            self.calls.borrow_mut().push("rm-merged (loop)".to_owned());
            return Ok(());
        }
        self.call("rm-merged".to_owned())
    }

    fn review_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("review {pr}"))?;
        write_task(&self.base, Kind::Pr, pr, None, REPO);
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

    fn queue(&self) -> PathBuf {
        jobs::queue_dir(&self.base())
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        }
    }

    /// Writes job file `name` with `text`, modified `age` seconds before [`START`].
    fn queue_job(&self, name: &str, text: &str, age: u64) -> PathBuf {
        fs::create_dir_all(self.queue()).unwrap();
        let path = self.queue().join(name);
        fs::write(&path, text).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(START - age))
            .unwrap();
        path
    }

    /// The result file `done/<name>`, parsed.
    fn result(&self, name: &str) -> Value {
        let path = jobs::done_dir(&self.base()).join(name);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no result {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// A finished issue task of `repo` whose finish opened PR `pr`.
    fn opened(&self, issue: u32, pr: u32, repo: &str) -> PathBuf {
        let dir = write_task(&self.base(), Kind::Issue, issue, None, repo);
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{repo}/pull/{pr}"));
        record::write(&dir, &rec).unwrap();
        dir
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
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// GitHub with no open issue and the open PRs `prs`.
fn github(prs: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    for &n in prs {
        gh = gh.with_pr(pr(n, PrState::Open));
    }
    gh
}

struct Run {
    result: Result<()>,
    out: String,
}

/// Runs the loop; `stop_after` asks a stop once that many phases ran.
fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub, stop_after: Option<usize>) -> Run {
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let stop = || stop_after.is_some_and(|n| script.calls().len() >= n);
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
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

// --- The job file ---

#[test]
fn the_four_shapes_parse() {
    assert_eq!(
        jobs::parse(r#"{"kind": "review", "pr": 465}"#),
        Ok(Job::Review { pr: 465 })
    );
    assert_eq!(
        jobs::parse(r#"{"kind": "fix", "pr": 465, "check_run": 42452742075}"#),
        Ok(Job::Fix {
            pr: 465,
            check_run: Some(42_452_742_075)
        })
    );
    assert_eq!(
        jobs::parse(r#"{"kind": "fix", "pr": 465}"#),
        Ok(Job::Fix {
            pr: 465,
            check_run: None
        })
    );
    assert_eq!(
        jobs::parse(r#"{"kind": "run", "issue": 521}"#),
        Ok(Job::Run { issue: 521 })
    );
    assert_eq!(
        jobs::parse(r#"{"kind": "rm-merged"}"#),
        Ok(Job::RmMerged {})
    );
}

#[test]
fn every_other_shape_is_refused() {
    for text in [
        "{not json",
        r#"{"kind": "spec", "file": "x.md"}"#,
        r#"{"kind":"review","pr":1,"force":true}"#,
        r#"{"kind":"review"}"#,
        r#"{"kind":"review","pr":"465"}"#,
        r#"{"kind":"rm-merged","yes":false}"#,
        r#"{"kind":"run","issue":-1}"#,
        r#"{"kind":"review","pr":0}"#,
        r#"{"kind":"run","issue":0}"#,
        r#"{"kind":"fix","pr":3,"check_run":0}"#,
        r#"{"pr":3}"#,
        r#"[]"#,
    ] {
        assert!(jobs::parse(text).is_err(), "{text} parsed");
    }
}

// --- The queue folder ---

#[test]
fn a_missing_queue_folder_creates_nothing_and_the_loop_goes_on_to_issues() {
    let f = fixture();
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[]), None);
    run.result.unwrap();
    assert!(run.out.contains("the queue is empty"), "{}", run.out);
    assert!(!f.queue().exists());
}

#[test]
fn an_empty_queue_folder_creates_nothing() {
    let f = fixture();
    fs::create_dir_all(f.queue()).unwrap();
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[]), None);
    run.result.unwrap();
    assert!(run.out.contains("the queue is empty"), "{}", run.out);
    assert_eq!(fs::read_dir(f.queue()).unwrap().count(), 0);
    assert!(script.calls().is_empty());
}

#[test]
fn jobs_run_oldest_first_and_ties_by_file_name() {
    let f = fixture();
    f.opened(1, 11, REPO);
    f.opened(2, 12, REPO);
    f.opened(3, 13, REPO);
    f.queue_job("a.json", r#"{"kind":"review","pr":11}"#, 10);
    f.queue_job("c.json", r#"{"kind":"review","pr":13}"#, 100);
    f.queue_job("b.json", r#"{"kind":"review","pr":12}"#, 100);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[11, 12, 13]), None)
        .result
        .unwrap();
    assert_eq!(script.calls(), ["review 12", "review 13", "review 11"]);
}

#[test]
fn a_file_not_ending_in_json_is_left_in_place() {
    let f = fixture();
    let tmp = f.queue_job("x.json.tmp", r#"{"kind":"rm-merged"}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[]), None).result.unwrap();
    assert!(tmp.is_file());
    assert!(script.calls().is_empty());
    assert!(!jobs::done_dir(&f.base()).exists());
}

#[test]
fn a_job_outside_the_shapes_is_refused_with_a_result_file_and_runs_nothing() {
    let f = fixture();
    f.opened(1, 11, REPO);
    let bad = [
        ("bad.json", "{not json"),
        ("unknown.json", r#"{"kind":"spec","file":"x.md"}"#),
        ("extra.json", r#"{"kind":"review","pr":11,"force":true}"#),
        ("missing.json", r#"{"kind":"review"}"#),
        ("type.json", r#"{"kind":"review","pr":"11"}"#),
    ];
    for (name, text) in bad {
        f.queue_job(name, text, 10);
    }
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[11]), None);
    run.result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    for (name, text) in bad {
        let result = f.result(name);
        assert_eq!(result["outcome"], "refused", "{name}: {result}");
        assert!(
            result["reason"].as_str().unwrap().contains("not a job"),
            "{result}"
        );
        assert!(!f.queue().join(name).exists());
        assert!(!jobs::running_dir(&f.base()).join(name).exists());
        if name == "bad.json" {
            assert_eq!(result["job_text"], text);
        } else {
            assert_eq!(result["job"], serde_json::from_str::<Value>(text).unwrap());
        }
    }
    assert!(run.out.contains("job extra.json: refused"), "{}", run.out);
}

// --- review and fix ---

#[test]
fn a_review_of_a_pr_with_a_live_record_runs_and_its_result_names_the_task() {
    let f = fixture();
    f.opened(1, 465, REPO);
    f.queue_job("r.json", r#"{"kind": "review", "pr": 465}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465]), None).result.unwrap();
    assert_eq!(script.calls(), ["review 465"]);
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "done", "{result}");
    assert_eq!(result["task"], "pr-465");
    assert_eq!(result["url"], format!("https://github.com/{REPO}/pull/465"));
    assert_eq!(result["job"]["pr"], 465);
    let started = result["started_at"].as_str().unwrap();
    assert!(started.ends_with('Z') && started.contains('T'), "{started}");
    assert!(result["ended_at"].as_str().unwrap() >= started);
    assert!(
        f.log_text().contains("| job review #465 |"),
        "{}",
        f.log_text()
    );
    assert_eq!(
        fs::read_dir(jobs::running_dir(&f.base())).unwrap().count(),
        0
    );
}

#[test]
fn a_review_of_a_pr_with_only_an_archived_record_runs() {
    let f = fixture();
    let dir = f.opened(1, 465, REPO);
    let archive = sbxm::task::merged::archive_dir(&f.base(), "issue-1");
    fs::create_dir_all(&archive).unwrap();
    fs::copy(dir.join("task.json"), archive.join("task.json")).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    f.queue_job("r.json", r#"{"kind": "review", "pr": 465}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465]), None).result.unwrap();
    assert_eq!(script.calls(), ["review 465"]);
    assert_eq!(f.result("r.json")["outcome"], "done");
}

#[test]
fn a_review_or_fix_of_a_pr_with_no_record_or_another_repos_is_refused() {
    let f = fixture();
    f.opened(1, 466, "other/repo");
    f.queue_job("none.json", r#"{"kind": "review", "pr": 465}"#, 20);
    f.queue_job("other.json", r#"{"kind": "fix", "pr": 466}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465, 466]), None)
        .result
        .unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    for (name, command) in [
        ("none.json", "review --pr 465"),
        ("other.json", "fix --pr 466"),
    ] {
        let result = f.result(name);
        assert_eq!(result["outcome"], "refused", "{result}");
        let reason = result["reason"].as_str().unwrap();
        assert!(reason.contains("didn't open it"), "{reason}");
        assert!(reason.contains(&format!("sbxm task {command}")), "{reason}");
    }
}

#[test]
fn a_review_of_a_merged_or_closed_pr_is_skipped() {
    let f = fixture();
    f.opened(1, 465, REPO);
    f.opened(2, 466, REPO);
    f.queue_job("merged.json", r#"{"kind": "review", "pr": 465}"#, 20);
    f.queue_job("closed.json", r#"{"kind": "fix", "pr": 466}"#, 10);
    let gh = github(&[])
        .with_pr(pr(465, PrState::Merged))
        .with_pr(pr(466, PrState::Closed));
    let script = Script::new(&f.base());
    run_loop(&f, &script, &gh, None).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    let merged = f.result("merged.json");
    assert_eq!(merged["outcome"], "skipped");
    assert!(merged["reason"].as_str().unwrap().contains("merged"));
    let closed = f.result("closed.json");
    assert_eq!(closed["outcome"], "skipped");
    assert!(closed["reason"].as_str().unwrap().contains("closed"));
}

#[test]
fn a_fix_job_runs_task_fix_with_or_without_its_check_run() {
    let f = fixture();
    f.opened(1, 465, REPO);
    f.queue_job("a.json", r#"{"kind": "fix", "pr": 465}"#, 20);
    f.queue_job(
        "b.json",
        r#"{"kind": "fix", "pr": 465, "check_run": 42452742075}"#,
        10,
    );
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465]), None).result.unwrap();
    assert_eq!(
        script.calls(),
        ["fix 465", "fix 465 --check-run 42452742075"]
    );
    assert_eq!(f.result("a.json")["outcome"], "done");
    assert_eq!(f.result("b.json")["outcome"], "done");
    assert!(
        f.log_text()
            .contains("| job fix #465 --check-run 42452742075 |"),
        "{}",
        f.log_text()
    );
}

#[test]
fn a_failed_job_is_failed_and_the_loop_goes_on_to_the_next_job() {
    let f = fixture();
    f.opened(1, 465, REPO);
    f.queue_job("a.json", r#"{"kind": "review", "pr": 465}"#, 20);
    f.queue_job("b.json", r#"{"kind": "rm-merged"}"#, 10);
    let script = Script::new(&f.base()).failing("review 465");
    let run = run_loop(&f, &script, &github(&[465]), None);
    run.result.unwrap();
    assert_eq!(script.calls(), ["review 465", "rm-merged"]);
    let failed = f.result("a.json");
    assert_eq!(failed["outcome"], "failed");
    assert!(
        failed["reason"]
            .as_str()
            .unwrap()
            .contains("review 465 failed")
    );
    assert_eq!(f.result("b.json")["outcome"], "done");
}

// --- run and rm-merged ---

#[test]
fn a_run_of_an_issue_not_labeled_ready_is_refused() {
    let f = fixture();
    f.queue_job("r.json", r#"{"kind": "run", "issue": 521}"#, 10);
    let gh = github(&[])
        .with_issue_text(issue_text(521))
        .with_open_issues(vec![open_issue(521, &["size:S"], "")]);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &gh, None).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "refused");
    assert!(
        result["reason"]
            .as_str()
            .unwrap()
            .contains("not labeled ready-for-worker"),
        "{result}"
    );
}

#[test]
fn a_run_of_a_ready_issue_is_taken_to_its_pr() {
    let f = fixture();
    f.queue_job("r.json", r#"{"kind": "run", "issue": 521}"#, 10);
    let gh = github(&[])
        .with_issue_text(issue_text(521))
        .with_pr(pr(621, PrState::Open))
        .with_open_issues(vec![open_issue(521, &[READY, "size:S"], "")]);
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &gh, None);
    run.result.unwrap();
    assert_eq!(script.calls()[..2], ["run 521", "finish 521"]);
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "done", "{result}");
    assert_eq!(result["task"], "issue-521");
    assert_eq!(result["url"], format!("https://github.com/{REPO}/pull/621"));
    assert!(
        f.log_text().contains("| job run #521 |"),
        "{}",
        f.log_text()
    );
}

#[test]
fn an_rm_merged_job_runs_task_rm_merged() {
    let f = fixture();
    f.queue_job("rm.json", r#"{"kind": "rm-merged"}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[]), None).result.unwrap();
    assert_eq!(script.calls(), ["rm-merged"]);
    let result = f.result("rm.json");
    assert_eq!(result["outcome"], "done");
    assert_eq!(result["task"], Value::Null);
}

// --- Results, stops and interrupted jobs ---

#[test]
fn a_result_name_already_in_done_is_not_overwritten() {
    let f = fixture();
    let done = jobs::done_dir(&f.base());
    fs::create_dir_all(&done).unwrap();
    fs::write(done.join("rm.json"), "earlier").unwrap();
    f.queue_job("rm.json", r#"{"kind": "rm-merged"}"#, 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[]), None).result.unwrap();
    assert_eq!(fs::read_to_string(done.join("rm.json")).unwrap(), "earlier");
    assert_eq!(f.result("rm-2.json")["outcome"], "done");
}

#[test]
fn a_stop_request_between_jobs_leaves_the_rest_queued() {
    let f = fixture();
    f.opened(1, 465, REPO);
    f.queue_job("a.json", r#"{"kind": "review", "pr": 465}"#, 20);
    let second = f.queue_job("b.json", r#"{"kind": "rm-merged"}"#, 10);
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[465]), Some(1));
    run.result.unwrap();
    assert_eq!(script.calls(), ["review 465"]);
    assert!(run.out.contains("stopped on request"), "{}", run.out);
    assert!(second.is_file());
    assert_eq!(f.result("a.json")["outcome"], "done");
}

#[test]
fn a_job_left_running_with_its_task_is_reported_interrupted_and_not_run_again() {
    let f = fixture();
    write_task(&f.base(), Kind::Pr, 465, None, REPO);
    let running = jobs::running_dir(&f.base());
    fs::create_dir_all(&running).unwrap();
    fs::write(
        running.join("r.json"),
        r#"{"job": {"kind": "review", "pr": 465}, "started_at": "2026-11-09T10:00:00Z", "task": "pr-465"}"#,
    )
    .unwrap();
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465]), None).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert!(!running.join("r.json").exists());
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "interrupted", "{result}");
    assert_eq!(result["task"], "pr-465");
    assert_eq!(result["started_at"], "2026-11-09T10:00:00Z");
    assert_eq!(result["job"]["kind"], "review");
    assert!(
        result["reason"].as_str().unwrap().contains("ready ok"),
        "{result}"
    );
    assert!(
        f.log_text().contains("| job review #465 |"),
        "{}",
        f.log_text()
    );
}

#[test]
fn a_job_left_running_before_its_task_is_reported_interrupted_without_one() {
    let f = fixture();
    let running = jobs::running_dir(&f.base());
    fs::create_dir_all(&running).unwrap();
    // Moved there, not yet marked: the job file as written.
    fs::write(running.join("r.json"), r#"{"kind": "run", "issue": 521}"#).unwrap();
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[]), None).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "interrupted");
    assert_eq!(result["task"], Value::Null);
    assert_eq!(result["next"], Value::Null);
    assert_eq!(result["job"]["issue"], 521);
}

#[test]
fn an_interrupted_task_not_done_names_the_command_that_continues_it() {
    let f = fixture();
    let dir = write_task(&f.base(), Kind::Issue, 521, None, REPO);
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Working;
    rec.status = Status::Failed;
    record::write(&dir, &rec).unwrap();
    let running = jobs::running_dir(&f.base());
    fs::create_dir_all(&running).unwrap();
    fs::write(
        running.join("r.json"),
        r#"{"job": {"kind": "run", "issue": 521}, "started_at": "2026-11-09T10:00:00Z", "task": "issue-521"}"#,
    )
    .unwrap();
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[]), None).result.unwrap();
    let result = f.result("r.json");
    assert_eq!(result["task"], "issue-521");
    assert_eq!(result["next"], "sbxm task resume --issue 521");
    assert!(
        result["reason"]
            .as_str()
            .unwrap()
            .contains("working failed"),
        "{result}"
    );
}

#[test]
fn a_queue_folder_that_is_a_file_refuses_the_loop_before_its_lock() {
    let f = fixture();
    fs::create_dir_all(f.base().join(".sbxm")).unwrap();
    fs::write(f.queue(), "not a folder").unwrap();
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[]), None);
    assert!(run.result.is_err(), "{}", run.out);
    assert!(!sbxm::task::queue::lock_path(&f.base()).exists());
}

#[cfg(unix)]
#[test]
fn a_job_file_that_cannot_be_read_is_refused_with_a_result_file() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let path = f.queue_job("locked.json", r#"{"kind": "rm-merged"}"#, 10);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&path).is_ok() {
        return; // root reads it anyway: nothing to check here
    }
    let script = Script::new(&f.base());
    let run = run_loop(&f, &script, &github(&[]), None);
    run.result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    let result = f.result("locked.json");
    assert_eq!(result["outcome"], "refused", "{result}");
    assert!(
        result["reason"].as_str().unwrap().contains("cannot read"),
        "{result}"
    );
    assert_eq!(
        fs::read_dir(jobs::running_dir(&f.base())).unwrap().count(),
        0
    );
}

#[test]
fn a_running_file_naming_a_path_instead_of_a_task_id_names_no_task() {
    let f = fixture();
    // A record outside the task folders, which the running file points at.
    let outside = f.base().join("elsewhere");
    write_task(&f.base(), Kind::Pr, 465, None, REPO);
    fs::create_dir_all(&outside).unwrap();
    fs::copy(
        record::task_dir(&f.base(), "pr-465").join("task.json"),
        outside.join("task.json"),
    )
    .unwrap();
    let running = jobs::running_dir(&f.base());
    fs::create_dir_all(&running).unwrap();
    fs::write(
        running.join("r.json"),
        r#"{"job": {"kind": "review", "pr": 465}, "started_at": "2026-11-09T10:00:00Z", "task": "../../elsewhere"}"#,
    )
    .unwrap();
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[465]), None).result.unwrap();
    let result = f.result("r.json");
    assert_eq!(result["outcome"], "interrupted");
    assert_eq!(result["task"], Value::Null, "{result}");
}
