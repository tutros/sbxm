//! Issue 530 (decision 311): `sbxm task loop` notifies the user only when they have something to
//! do: (a) a PR awaiting their merge, (b) a queued job that failed, stopped, was refused or
//! interrupted, (c) a stop that needs them, (d) `main`'s second failure pausing arming, (e) a
//! question a task handed back. Each event once, ever, through `notified.json`. The phases are
//! scripted, the PRs are `FakeGitHub`'s and the notifier is `FakeNotifier`.

mod common;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState, RunStatus, WorkflowRun};
use sbxm::notify::{FakeNotifier, Notification, shown_path};
use sbxm::task::pipeline::MissingSecret;
use sbxm::task::record::{
    self, Kind, NewTask, PrBranch, Process, Record, ReviewResult, Stage, Status,
};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const PUSHED: &str = "1111111111111111111111111111111111111111";
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Plays `task run`, `task finish`, `task file-findings` and `task rm --merged`: each task reaches
/// ready with a review of [`HEAD`] at `risk` with `must_fix`, and finishes into PR `100 + issue`.
struct Script {
    base: PathBuf,
    risk: Level,
    must_fix: u32,
    /// A "Needs a decision" the worker's `result.md` hands back.
    question: Option<&'static str>,
    /// A question finding the reviewer's `review.md` asks, as `### Q-1 - <it>`.
    review_question: Option<&'static str>,
    /// `task run` fails on a missing secret.
    no_secret: bool,
    clock: Cell<u64>,
}

impl Script {
    fn new(base: &Path, risk: Level) -> Self {
        Self {
            base: base.to_owned(),
            risk,
            must_fix: 0,
            question: None,
            review_question: None,
            no_secret: false,
            clock: Cell::new(START),
        }
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        if self.no_secret {
            return Err(MissingSecret::new(
                "the secret `anthropic` is not set; run `sbx secret set`",
            )
            .into());
        }
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
            commit: Some(HEAD.to_owned()),
            must_fix: self.must_fix,
            risk: self.risk,
            ..ReviewResult::default()
        });
        record::write(&dir, &rec).unwrap();
        if let Some(question) = self.question {
            fs::write(
                dir.join("result.md"),
                format!(
                    "# Result\n\n## Choices made\n\nnone\n\n## Needs a decision\n\n{question}\n"
                ),
            )
            .unwrap();
        }
        if let Some(question) = self.review_question {
            fs::write(
                dir.join("review.md"),
                format!(
                    "Must-fix findings: 0\n\nRisk: low\n\n## Questions\n\n### Q-1 - {question}\n\n\
                     **Where:** `src/a.rs:1`\n**What happens:** it isn't said\n**Why it matters:** \
                     decision 1\n**Recommendation:** keep it\n"
                ),
            )
            .unwrap();
        }
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

    fn file_findings(&self, _: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }

    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    /// A queued review of PR `pr`: its PR task `pr-<n>`, reviewed low, asking a question.
    fn review_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let dir = record::task_dir(&self.base, &format!("pr-{pr}"));
        let mut rec = Record::new(
            &NewTask {
                kind: Kind::Pr,
                number: pr,
                repo: REPO,
                title: "t",
                base: "main",
                branch: &format!("b{pr}"),
                config_hash: "h",
                id: None,
            },
            START,
            Process::new(1, 0),
        );
        rec.stage = Stage::Ready;
        rec.status = Status::Ok;
        record::write(&dir, &rec).unwrap();
        fs::write(
            dir.join("review.md"),
            "Must-fix findings: 0\n\n## Questions\n\n### Q-1 - Is the old name still used?\n\n\
             **Where:** `src/a.rs:1`\n**What happens:** unclear\n**Why it matters:** decision 1\n\
             **Recommendation:** check\n",
        )
        .unwrap();
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

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        }
    }

    fn queue(&self, name: &str, text: &str) {
        let dir = self.base().join(".sbxm").join("queue");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    fn shown_keys(&self) -> Option<Vec<String>> {
        let text = fs::read_to_string(shown_path(&self.base())).ok()?;
        Some(serde_json::from_str(&text).unwrap())
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

/// GitHub with the open issues `issues`, each finished into PR `100 + issue` at `head`.
fn github(issues: &[u32], head: &str) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN);
    let mut open = Vec::new();
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n)).with_pr(pr(100 + n, head));
        open.push(open_issue(n, &[READY, "size:S"], ""));
    }
    gh.with_open_issues(open)
}

struct Ran {
    result: Result<()>,
    out: String,
    warn: String,
}

/// Runs the loop with `notifier`; a loop that sleeps (a pause, a `--wait`) is stopped by the user
/// at its first look back.
fn run_loop(
    f: &Fixture,
    opts: &Options,
    script: &Script,
    gh: &FakeGitHub,
    notifier: &FakeNotifier,
) -> Ran {
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
    let result = task_loop::run_notifying(
        &f.env.config_dir(),
        opts,
        script,
        gh,
        &Probe,
        &env,
        notifier,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn titles(shown: &[Notification]) -> Vec<String> {
    shown.iter().map(|n| n.title.clone()).collect()
}

// --- (a) a PR awaiting the user's merge ---

#[test]
fn a_medium_risk_pr_notifies_once_and_its_click_opens_the_pr() {
    let f = fixture();
    let gh = github(&[3], HEAD);
    let script = Script::new(&f.base(), Level::Medium);
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (medium)"]);
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
    assert!(shown[0].body.contains("risk medium"), "{}", shown[0].body);
    assert!(shown[0].body.contains("next: "), "{}", shown[0].body);
    assert_eq!(shown[0].key, format!("pr:103:{HEAD}"));
}

#[test]
fn the_scheduled_loop_notifies_a_medium_risk_pr_once_with_a_click_on_the_pr() {
    let f = fixture();
    let gh = github(&[3], HEAD);
    let script = Script::new(&f.base(), Level::High);
    let notifier = FakeNotifier::default();
    let opts = Options {
        wait: true,
        ..f.opts()
    };

    run_loop(&f, &opts, &script, &gh, &notifier).result.unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (high)"]);
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
}

#[test]
fn a_medium_risk_pr_a_loop_task_continued_notifies_too() {
    let f = fixture();
    // An earlier loop task continued PR #140 (`PR: #140`) and finished into it.
    let script = Script::new(&f.base(), Level::Medium);
    script.run(40, &mut Vec::new(), &mut Vec::new()).unwrap();
    let dir = record::task_dir(&f.base(), "issue-40");
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/140"));
    rec.continues = Some(PrBranch {
        pr: 140,
        branch: "feature".into(),
        base: HEAD.into(),
    });
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    record::write(&dir, &rec).unwrap();
    let gh = github(&[], HEAD).with_pr(pr(140, PUSHED));
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["PR #140 awaits your merge (medium)"]);
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/140");
}

#[test]
fn a_draft_pr_notifies_with_its_must_fix() {
    let f = fixture();
    let gh = github(&[3], HEAD);
    let mut script = Script::new(&f.base(), Level::Low);
    script.must_fix = 2;
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (draft)"]);
    assert!(shown[0].body.contains("2 must-fix"), "{}", shown[0].body);
    assert!(
        shown[0].body.contains("sbxm task status --issue 3"),
        "{}",
        shown[0].body
    );
}

#[test]
fn a_clean_low_risk_pr_auto_merged_stays_quiet() {
    let f = fixture();
    let gh = github(&[3], HEAD);
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &gh, &notifier);

    ran.result.unwrap();
    assert!(ran.out.contains("auto-merge armed"), "{}", ran.out);
    assert!(notifier.shown().is_empty());
    // An empty queue stops the loop quietly too.
    assert!(ran.out.contains("the queue is empty"), "{}", ran.out);
}

#[test]
fn the_same_pr_and_head_seen_at_three_steps_notifies_once() {
    let f = fixture();
    // #3's PR is seen before #4, before #5 and at the stop.
    let gh = github(&[3, 4, 5], HEAD);
    let script = Script::new(&f.base(), Level::Medium);
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    let titles = titles(&notifier.shown());
    assert_eq!(
        titles.iter().filter(|t| t.starts_with("PR #103 ")).count(),
        1,
        "{titles:?}"
    );
    assert_eq!(titles.len(), 3, "{titles:?}");
}

#[test]
fn a_restart_does_not_repeat_and_a_new_head_after_a_fix_round_notifies_again() {
    let f = fixture();
    let script = Script::new(&f.base(), Level::Medium);
    let first = FakeNotifier::default();
    run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &first)
        .result
        .unwrap();
    assert_eq!(first.shown().len(), 1);

    // Restarted, the PR still at its head: notified.json holds the key.
    let again = FakeNotifier::default();
    run_loop(&f, &f.opts(), &script, &github(&[], HEAD), &again)
        .result
        .unwrap();
    assert!(again.shown().is_empty(), "{:?}", again.shown());

    // A fix round pushed a new head, still medium.
    let pushed = FakeNotifier::default();
    let gh = github(&[], HEAD).with_pr(pr(103, PUSHED));
    run_loop(&f, &f.opts(), &script, &gh, &pushed)
        .result
        .unwrap();
    let shown = pushed.shown();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (medium)"]);
    assert_eq!(shown[0].key, format!("pr:103:{PUSHED}"));
}

#[test]
fn an_unreadable_notified_file_is_a_warning_and_read_as_empty() {
    let f = fixture();
    let path = shown_path(&f.base());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ broken").unwrap();
    let script = Script::new(&f.base(), Level::Medium);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier);

    ran.result.unwrap();
    assert!(ran.warn.contains("notified.json"), "{}", ran.warn);
    assert_eq!(notifier.shown().len(), 1);
    assert_eq!(f.shown_keys().unwrap(), [format!("pr:103:{HEAD}")]);
}

#[test]
fn toasts_false_notifies_nothing_and_writes_no_file() {
    let f = fixture();
    let config = f.env.config_dir().join("config.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{text}\n[notify]\ntoasts = false\n")).unwrap();
    let script = Script::new(&f.base(), Level::Medium);
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier)
        .result
        .unwrap();

    assert!(notifier.shown().is_empty());
    assert!(!shown_path(&f.base()).exists());
}

#[test]
fn toasts_true_or_absent_notifies() {
    for setting in ["\n[notify]\ntoasts = true\n", ""] {
        let f = fixture();
        let config = f.env.config_dir().join("config.toml");
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, format!("{text}{setting}")).unwrap();
        let script = Script::new(&f.base(), Level::Medium);
        let notifier = FakeNotifier::default();

        run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier)
            .result
            .unwrap();

        assert_eq!(notifier.shown().len(), 1, "{setting:?}");
    }
}

#[test]
fn a_notifier_that_fails_is_one_warning_the_loop_goes_on_and_the_key_is_recorded() {
    let f = fixture();
    let script = Script::new(&f.base(), Level::Medium);
    let notifier = FakeNotifier::failing();

    let ran = run_loop(&f, &f.opts(), &script, &github(&[3, 4], HEAD), &notifier);

    ran.result.unwrap();
    assert_eq!(
        ran.warn.matches("warning: the notification").count(),
        2,
        "one per notification: {}",
        ran.warn
    );
    assert!(ran.out.contains("#4"), "the loop went on: {}", ran.out);
    let keys = f.shown_keys().unwrap();
    assert!(keys.contains(&format!("pr:103:{HEAD}")), "{keys:?}");
    assert!(keys.contains(&format!("pr:104:{HEAD}")), "{keys:?}");
}

// --- (b) a queued job that didn't end done or skipped ---

#[test]
fn a_refused_job_notifies_with_its_issue_as_the_click() {
    let f = fixture();
    // #529 isn't labeled, so the job is refused.
    f.queue("j1.json", r#"{"kind": "run", "issue": 529}"#);
    let gh = github(&[], HEAD).with_issue_text(issue_text(529));
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &gh, &notifier);

    ran.result.unwrap();
    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["Job run #529 refused"], "{}", ran.out);
    assert_eq!(shown[0].click, "https://github.com/o/r/issues/529");
    assert_eq!(shown[0].key, "job:j1.json");
}

#[test]
fn a_job_ending_done_stays_quiet() {
    let f = fixture();
    f.queue("j1.json", r#"{"kind": "rm-merged"}"#);
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &github(&[], HEAD), &notifier);

    ran.result.unwrap();
    assert!(ran.out.contains("job rm-merged: done"), "{}", ran.out);
    assert!(notifier.shown().is_empty());
}

#[test]
fn an_unreadable_job_refused_has_the_loop_log_as_its_click() {
    let f = fixture();
    f.queue("bad.json", "not json");
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &github(&[], HEAD), &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].title.ends_with(" refused"), "{}", shown[0].title);
    assert!(shown[0].click.starts_with("file://"), "{}", shown[0].click);
    assert!(shown[0].click.contains("loop-logs"), "{}", shown[0].click);
}

// --- (c) a stop that needs the user ---

#[test]
fn a_stop_on_a_missing_secret_notifies_with_the_log_as_its_click() {
    let f = fixture();
    let mut script = Script::new(&f.base(), Level::Low);
    script.no_secret = true;
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier);

    assert_eq!(task_loop::exit_code(&ran.result), 3);
    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["Loop stopped: a secret is missing"]);
    assert!(
        shown[0].body.contains("sbx secret set"),
        "{}",
        shown[0].body
    );
    assert!(shown[0].click.starts_with("file://"), "{}", shown[0].click);
    assert_eq!(shown[0].key, format!("stop:{START}:secret"));
}

#[test]
fn a_loop_stopped_on_request_stays_quiet() {
    let f = fixture();
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();
    let opts = Options {
        wait: true,
        ..f.opts()
    };

    let ran = run_loop(&f, &opts, &script, &github(&[], HEAD), &notifier);

    ran.result.unwrap();
    assert!(ran.out.contains("stopped on request"), "{}", ran.out);
    assert!(notifier.shown().is_empty());
}

// --- (d) main's second failure pauses arming ---

#[test]
fn mains_second_failure_notifies_once_with_the_run_as_its_click() {
    let f = fixture();
    let gh = github(&[3, 4], HEAD).with_runs(
        MAIN,
        vec![WorkflowRun {
            id: 9,
            workflow: "ci".into(),
            attempt: 2,
            status: RunStatus::Failed,
            url: "https://github.com/o/r/actions/runs/9".into(),
            actor: "alice".into(),
        }],
    );
    let script = Script::new(&f.base(), Level::Low);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &gh, &notifier);

    ran.result.unwrap();
    let shown = notifier.shown();
    // Paused, the two clean low-risk PRs aren't armed, so they await the user but stay quiet.
    assert_eq!(
        titles(&shown),
        ["main is red: auto-merge paused"],
        "{}",
        ran.out
    );
    assert_eq!(shown[0].click, "https://github.com/o/r/actions/runs/9");
    assert!(shown[0].body.contains("--clear-pause"), "{}", shown[0].body);
    assert_eq!(shown[0].key, format!("main:{MAIN}"));
}

// --- (e) a question handed back ---

#[test]
fn a_question_handed_back_notifies_with_its_pr_as_the_click() {
    let f = fixture();
    let mut script = Script::new(&f.base(), Level::Low);
    script.question = Some("- Should `<x>` & \"y\" be kept? Options: keep or drop.");
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["Task issue-3 has a question for you"]);
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
    assert!(
        shown[0].body.starts_with("Should `<x>` & \"y\" be kept?"),
        "{}",
        shown[0].body
    );
    assert!(
        shown[0].key.starts_with("question:issue-3:"),
        "{}",
        shown[0].key
    );
}

// --- no notifier ---

#[test]
fn the_loop_without_a_notifier_writes_no_notified_file() {
    let f = fixture();
    let script = Script::new(&f.base(), Level::Medium);
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let now = || script.clock.get();
    let env = Env {
        now: &now,
        sleep: &|_| {},
        stop_requested: &|| false,
    };

    task_loop::run(
        &f.env.config_dir(),
        &f.opts(),
        &script,
        &github(&[3], HEAD),
        &Probe,
        &env,
        &mut out,
        &mut warn,
    )
    .unwrap();

    assert!(!shown_path(&f.base()).exists());
}

// --- review of issue 530: a later fix's risk, and the reviewer's questions ---

/// A finished loop task `issue-3` reviewed low at [`HEAD`] into PR #103, and the resolve task of
/// PR #103 started at [`HEAD`] whose review covered [`PUSHED`] at `risk`.
fn low_task_with_a_fix(f: &Fixture, risk: Level) {
    let script = Script::new(&f.base(), Level::Low);
    script.run(3, &mut Vec::new(), &mut Vec::new()).unwrap();
    script.finish(3, &mut Vec::new(), &mut Vec::new()).unwrap();
    let dir = record::task_dir(&f.base(), "issue-3");
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
    record::write(&dir, &rec).unwrap();
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
    fix.review = Some(ReviewResult {
        round: 1,
        commit: Some(PUSHED.to_owned()),
        must_fix: 0,
        risk,
        ..ReviewResult::default()
    });
    record::write(&record::task_dir(&f.base(), &id), &fix).unwrap();
}

#[test]
fn a_low_risk_pr_left_unarmed_by_a_medium_risk_fix_notifies_once_for_its_new_head() {
    let f = fixture();
    low_task_with_a_fix(&f, Level::Medium);
    let script = Script::new(&f.base(), Level::Low);
    let gh = github(&[], HEAD).with_pr(pr(103, PUSHED));
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (medium)"]);
    assert_eq!(shown[0].key, format!("pr:103:{PUSHED}"));
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");

    // Restarted at the same head: not again.
    let again = FakeNotifier::default();
    let gh = github(&[], HEAD).with_pr(pr(103, PUSHED));
    run_loop(&f, &f.opts(), &script, &gh, &again)
        .result
        .unwrap();
    assert!(again.shown().is_empty(), "{:?}", again.shown());
}

#[test]
fn a_low_risk_fix_of_a_low_risk_pr_stays_quiet() {
    let f = fixture();
    low_task_with_a_fix(&f, Level::Low);
    let script = Script::new(&f.base(), Level::Low);
    let gh = github(&[], HEAD).with_pr(pr(103, PUSHED));
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &gh, &notifier)
        .result
        .unwrap();

    assert!(notifier.shown().is_empty(), "{:?}", notifier.shown());
}

#[test]
fn a_reviewers_question_notifies_once_naming_the_task_and_opening_its_pr() {
    let f = fixture();
    let mut script = Script::new(&f.base(), Level::Low);
    script.review_question = Some("Needs a decision: keep the old setting?");
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(titles(&shown), ["Task issue-3 has a question for you"]);
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
    assert!(
        shown[0].body.contains("keep the old setting?"),
        "{}",
        shown[0].body
    );
    assert!(shown[0].body.contains("Q-1"), "{}", shown[0].body);
    assert!(
        shown[0].key.starts_with("question:issue-3:"),
        "{}",
        shown[0].key
    );
}

#[test]
fn a_workers_and_a_reviewers_question_of_one_task_notify_once_each() {
    let f = fixture();
    let mut script = Script::new(&f.base(), Level::Low);
    script.question = Some("- Which name?");
    script.review_question = Some("Keep the old setting?");
    let notifier = FakeNotifier::default();

    run_loop(&f, &f.opts(), &script, &github(&[3], HEAD), &notifier)
        .result
        .unwrap();

    let shown = notifier.shown();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_ne!(shown[0].key, shown[1].key);
    assert!(shown.iter().any(|n| n.body.starts_with("Which name?")));
    assert!(
        shown
            .iter()
            .any(|n| n.body.contains("Keep the old setting?"))
    );
}

#[test]
fn a_reviewers_question_in_a_queued_review_job_notifies_with_the_pr_as_the_click() {
    let f = fixture();
    // PR #103 was opened by a loop task, so a review job of it runs.
    let script = Script::new(&f.base(), Level::Low);
    script.run(3, &mut Vec::new(), &mut Vec::new()).unwrap();
    script.finish(3, &mut Vec::new(), &mut Vec::new()).unwrap();
    f.queue("r1.json", r#"{"kind": "review", "pr": 103}"#);
    let gh = github(&[], HEAD)
        .with_pr(pr(103, HEAD))
        .with_auto_merge(103);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &f.opts(), &script, &gh, &notifier);

    ran.result.unwrap();
    let shown = notifier.shown();
    assert_eq!(
        titles(&shown),
        ["Task pr-103 has a question for you"],
        "{}",
        ran.out
    );
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
    assert!(
        shown[0].body.contains("Q-1 Is the old name still used?"),
        "{}",
        shown[0].body
    );
}
