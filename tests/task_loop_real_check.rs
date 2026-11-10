//! Issue 547 (#460 guardrail 3): an issue whose body names a real check (`**Real check:** ...`)
//! keeps its PR unarmed, at any risk, until the PR's head has the commit status `sbxm/real-check`
//! = `success` (the latest of that context). Both arm paths of `task loop` (a new PR, and arming
//! again after a resolve) refuse with `real check not recorded` or `real check failed`, in the
//! "awaits you" line and the log row, and a low-risk PR held this way gets a toast once per head.
//! The phases are scripted, the PRs are `FakeGitHub`'s and the notifier is `FakeNotifier`.

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
use sbxm::github::{
    CommitStatus, GitHubBackend, Mergeable, PrFile, PrFiles, PrInfo, PrState, StatusState,
};
use sbxm::notify::{FakeNotifier, Notification};
use sbxm::task::loop_pause::{self, Pause};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
/// The PRs' heads, the commit every in-task review covered.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// A PR's head after a resolve pushed onto it.
const PUSHED: &str = "1111111111111111111111111111111111111111";
/// An older commit of a PR.
const OLDER: &str = "2222222222222222222222222222222222222222";
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const CHECK: &str = "run the Windows tests 50 times; a pass is 50 green runs";

/// An issue body that names [`CHECK`].
fn needs_check() -> String {
    format!("**Size:** M\n\n**Real check:** {CHECK}\n\nDo it.\n")
}

/// Plays `task run` (a task at ready, its review of [`HEAD`] at `risk`, clean, with the issue
/// text `body` saved as the start saves it), `task finish` (PR `100 + issue`) and `task fix --pr
/// N` (a clean low-risk resolve that pushes [`PUSHED`]).
struct Script<'a> {
    base: PathBuf,
    gh: &'a FakeGitHub,
    risk: Level,
    /// The risk of every resolve's review.
    fix_risk: Level,
    /// The issue's body; no `issue.md` at all for `None`.
    body: Option<String>,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
}

impl<'a> Script<'a> {
    fn new(base: &Path, gh: &'a FakeGitHub, body: Option<String>) -> Self {
        Self {
            base: base.to_owned(),
            gh,
            risk: Level::Low,
            fix_risk: Level::Low,
            body,
            calls: RefCell::default(),
            clock: Cell::new(START),
        }
    }

    fn risk(mut self, risk: Level) -> Self {
        self.risk = risk;
        self
    }

    fn fix_risk(mut self, risk: Level) -> Self {
        self.fix_risk = risk;
        self
    }
}

/// A task record of `kind`, at ready with a clean review of `commit` at `risk`; for an issue
/// task, `body` saved as its `issue.md`.
fn write_task(
    base: &Path,
    kind: Kind,
    number: u32,
    id: Option<&str>,
    (risk, commit): (Level, &str),
    body: Option<&str>,
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
        commit: Some(commit.to_owned()),
        must_fix: 0,
        risk,
        not_run: Some(Vec::new()),
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
    if let Some(body) = body {
        fs::write(
            dir.join("issue.md"),
            format!("title:\tFix {number}\nstate:\tOPEN\nlabels:\tsize:M\n--\n{body}"),
        )
        .unwrap();
    }
    (dir, rec)
}

impl Steps for Script<'_> {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        self.clock.set(self.clock.get() + 300);
        write_task(
            &self.base,
            Kind::Issue,
            issue,
            None,
            (self.risk, HEAD),
            self.body.as_deref(),
        );
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
        self.calls.borrow_mut().push(format!("fix {pr}"));
        self.clock.set(self.clock.get() + 200);
        let head = self.gh.pr(REPO, pr)?.head_oid;
        let id = record::fix_id(pr, &head).unwrap();
        let (dir, mut rec) = write_task(
            &self.base,
            Kind::Fix,
            pr,
            Some(&id),
            (self.fix_risk, PUSHED),
            None,
        );
        rec.stage = Stage::Finished;
        record::write(&dir, &rec).unwrap();
        self.gh.set_pr_head(pr, PUSHED);
        self.gh.set_mergeable(pr, Mergeable::Mergeable);
        Ok(())
    }

    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
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

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        }
    }

    /// A task of issue `issue` a loop finished into PR `100 + issue`, its clean low-risk review
    /// of [`HEAD`], its issue text `body`.
    fn loop_pr(&self, issue: u32, body: &str) {
        self.loop_pr_at(issue, body, Level::Low);
    }

    /// [`Self::loop_pr`] with its own task's review at `risk`.
    fn loop_pr_at(&self, issue: u32, body: &str, risk: Level) {
        let (dir, mut rec) = write_task(
            &self.base(),
            Kind::Issue,
            issue,
            None,
            (risk, HEAD),
            Some(body),
        );
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
        record::write(&dir, &rec).unwrap();
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

/// GitHub with the open issues `issues`, each finished into PR `100 + issue` at [`HEAD`], and the
/// PRs `prs` at [`HEAD`]. Every issue is size M and every PR changes one source file, so a
/// medium-risk PR has no other reason not to be armed.
fn github(issues: &[u32], prs: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN);
    let mut open = Vec::new();
    let files = PrFiles {
        changed_files: 1,
        files: vec![PrFile {
            filename: "src/a.rs".into(),
            previous_filename: None,
        }],
    };
    for &n in issues {
        let mut text = issue_text(n);
        text.text = format!("title:\tFix {n}\nstate:\tOPEN\nlabels:\tsize:M\n--\nbody\n");
        gh = gh
            .with_issue_text(text)
            .with_pr(pr(100 + n, HEAD))
            .with_pr_files(100 + n, files.clone());
        open.push(open_issue(n, &[READY, "size:M"], ""));
    }
    for &n in prs {
        gh = gh.with_pr(pr(n, HEAD)).with_pr_files(n, files.clone());
    }
    gh.with_open_issues(open)
}

/// Posts `state` for `sbxm/real-check` on `sha`, as `sbxm task real-check` does.
fn post(gh: &FakeGitHub, sha: &str, state: StatusState) {
    gh.commit_status(
        REPO,
        sha,
        &CommitStatus {
            state,
            context: "sbxm/real-check".into(),
            description: "by hand".into(),
        },
    )
    .unwrap();
}

struct Ran {
    result: Result<()>,
    out: String,
}

fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub, notifier: &FakeNotifier) -> Ran {
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
        &f.opts(),
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
    }
}

fn arms(gh: &FakeGitHub) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrArmAutoMerge(..)))
        .collect()
}

fn titles(shown: &[Notification]) -> Vec<String> {
    shown.iter().map(|n| n.title.clone()).collect()
}

// --- A new PR (the first arm path) ---

/// Runs the loop on issue 3, a low-risk task whose issue has `body`, after `statuses` were posted
/// on PR #103's commits.
fn new_pr(body: Option<String>, statuses: &[(&str, StatusState)]) -> (Ran, Vec<GhCall>, String) {
    let f = fixture();
    let gh = github(&[3], &[]);
    for &(sha, state) in statuses {
        post(&gh, sha, state);
    }
    let script = Script::new(&f.base(), &gh, body);
    let notifier = FakeNotifier::default();
    let ran = run_loop(&f, &script, &gh, &notifier);
    let log = f.log_text();
    (ran, arms(&gh), log)
}

#[test]
fn a_required_check_with_no_status_on_the_head_is_not_armed() {
    let (ran, arms, log) = new_pr(Some(needs_check()), &[]);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .contains("PR #103 (Fix 3) awaits you: auto-merge not armed: real check not recorded"),
        "{}",
        ran.out
    );
    assert!(
        log.contains("PR #103, clean, auto-merge not armed: real check not recorded"),
        "{log}"
    );
}

#[test]
fn success_on_the_head_is_armed() {
    let (ran, arms, _) = new_pr(Some(needs_check()), &[(HEAD, StatusState::Success)]);
    ran.result.unwrap();
    assert_eq!(
        arms,
        [GhCall::PrArmAutoMerge(REPO.into(), 103, HEAD.into())]
    );
}

#[test]
fn failure_on_the_head_is_not_armed_and_says_failed() {
    let (ran, arms, log) = new_pr(Some(needs_check()), &[(HEAD, StatusState::Failure)]);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .contains("awaits you: auto-merge not armed: real check failed"),
        "{}",
        ran.out
    );
    assert!(log.contains("not armed: real check failed"), "{log}");
}

#[test]
fn success_only_on_an_older_commit_is_not_armed() {
    let (ran, arms, log) = new_pr(Some(needs_check()), &[(OLDER, StatusState::Success)]);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(log.contains("not armed: real check not recorded"), "{log}");
}

#[test]
fn failure_then_success_on_the_head_is_armed() {
    let (ran, arms, _) = new_pr(
        Some(needs_check()),
        &[(HEAD, StatusState::Failure), (HEAD, StatusState::Success)],
    );
    ran.result.unwrap();
    assert_eq!(arms.len(), 1, "{}", ran.out);
}

#[test]
fn success_then_failure_on_the_head_is_not_armed() {
    let (ran, arms, log) = new_pr(
        Some(needs_check()),
        &[(HEAD, StatusState::Success), (HEAD, StatusState::Failure)],
    );
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(log.contains("not armed: real check failed"), "{log}");
}

#[test]
fn a_low_risk_pr_with_no_required_check_is_armed_as_today() {
    for body in [
        Some("**Size:** S\n\nDo it.\n".to_owned()),
        Some("**Real check:** none\n".to_owned()),
        Some("**Real check:** NONE\n".to_owned()),
        Some("```\n**Real check:** run it\n```\n".to_owned()),
        None,
    ] {
        let (ran, arms, _) = new_pr(body.clone(), &[]);
        ran.result.unwrap();
        assert_eq!(
            arms,
            [GhCall::PrArmAutoMerge(REPO.into(), 103, HEAD.into())],
            "{body:?}: {}",
            ran.out
        );
    }
}

// --- Arming again after a resolve (the second arm path) ---

/// PR #101, armed and conflicting, of a low-risk task whose issue names the check, after
/// `statuses` were posted; a clean low-risk resolve pushes [`PUSHED`] onto it.
fn resolved(statuses: &[(&str, StatusState)]) -> (Ran, Vec<GhCall>, String) {
    let f = fixture();
    f.loop_pr(1, &needs_check());
    let gh = github(&[], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101);
    for &(sha, state) in statuses {
        post(&gh, sha, state);
    }
    let script = Script::new(&f.base(), &gh, None);
    let notifier = FakeNotifier::default();
    let ran = run_loop(&f, &script, &gh, &notifier);
    assert_eq!(script.calls.borrow().clone(), ["fix 101"]);
    let log = f.log_text();
    (ran, arms(&gh), log)
}

#[test]
fn a_resolve_moves_the_head_so_success_on_the_old_head_is_not_armed_again() {
    let (ran, arms, log) = resolved(&[(HEAD, StatusState::Success)]);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .contains("PR #101 (Fix 1) awaits you: auto-merge not armed: real check not recorded"),
        "{}",
        ran.out
    );
    assert!(log.contains("not armed: real check not recorded"), "{log}");
}

#[test]
fn success_on_the_resolves_head_is_armed_again() {
    let (ran, arms, _) = resolved(&[(PUSHED, StatusState::Success)]);
    ran.result.unwrap();
    assert_eq!(
        arms,
        [GhCall::PrArmAutoMerge(REPO.into(), 101, PUSHED.into())],
        "{}",
        ran.out
    );
}

#[test]
fn failure_on_the_resolves_head_is_not_armed_again_and_says_failed() {
    let (ran, arms, log) = resolved(&[(PUSHED, StatusState::Failure)]);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(log.contains("not armed: real check failed"), "{log}");
}

// --- Review S-1: a refusal that comes first still names the real check ---

/// Runs the loop on issue 3, its task's review at `risk`, its issue naming the check, after
/// `statuses` were posted on PR #103's head; with `paused`, on a loop paused on a red `main`.
fn held_first(risk: Level, statuses: &[StatusState], paused: bool) -> (Ran, Vec<GhCall>, String) {
    let f = fixture();
    if paused {
        loop_pause::write(
            &f.base(),
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
    let gh = github(&[3], &[]);
    for &state in statuses {
        post(&gh, HEAD, state);
    }
    let script = Script::new(&f.base(), &gh, Some(needs_check())).risk(risk);
    let ran = run_loop(&f, &script, &gh, &FakeNotifier::default());
    let log = f.log_text();
    (ran, arms(&gh), log)
}

#[test]
fn a_medium_risk_pr_names_its_real_check_not_recorded() {
    let (ran, arms, log) = held_first(Level::Medium, &[], false);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out.contains(
            "PR #103 (Fix 3) awaits you: auto-merge not armed: real check not recorded; risk medium"
        ),
        "{}",
        ran.out
    );
    assert!(
        log.contains("PR #103, clean, auto-merge not armed: real check not recorded; risk medium"),
        "{log}"
    );
}

#[test]
fn a_medium_risk_pr_names_its_real_check_failed() {
    let (ran, arms, log) = held_first(Level::Medium, &[StatusState::Failure], false);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        log.contains("not armed: real check failed; risk medium"),
        "{log}"
    );
}

#[test]
fn a_medium_risk_pr_with_success_on_its_head_says_only_its_risk() {
    let (ran, arms, log) = held_first(Level::Medium, &[StatusState::Success], false);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out.contains("PR #103 (Fix 3) awaits you: risk medium"),
        "{}",
        ran.out
    );
    assert!(!log.contains("real check"), "{log}");
}

#[test]
fn a_paused_loop_still_names_a_low_risk_prs_real_check() {
    let (ran, arms, log) = held_first(Level::Low, &[], true);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        log.contains("not armed: paused on a red main; real check not recorded"),
        "{log}"
    );
}

/// PR #101 of a task whose own review was `own`, armed when `armed`, resolved by a fix whose
/// review is `fix`; nothing posted on any commit.
fn resolved_held(own: Level, armed: bool, fix: Level) -> (Ran, Vec<GhCall>, String) {
    let f = fixture();
    f.loop_pr_at(1, &needs_check(), own);
    let mut gh = github(&[], &[101]).with_mergeable(101, Mergeable::Conflicting);
    if armed {
        gh = gh.with_auto_merge(101);
    }
    let script = Script::new(&f.base(), &gh, None).fix_risk(fix);
    let ran = run_loop(&f, &script, &gh, &FakeNotifier::default());
    let log = f.log_text();
    (ran, arms(&gh), log)
}

#[test]
fn a_medium_risk_resolve_names_the_real_check() {
    let (ran, arms, log) = resolved_held(Level::Low, true, Level::Medium);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(
        ran.out.contains(
            "PR #101 (Fix 1) awaits you after its resolve: auto-merge not armed: real check not \
             recorded"
        ),
        "{}",
        ran.out
    );
    assert!(
        log.contains("not armed: real check not recorded; risk medium"),
        "{log}"
    );
}

#[test]
fn a_resolve_of_a_pr_its_own_review_kept_unarmed_names_the_real_check() {
    let (ran, arms, log) = resolved_held(Level::Medium, false, Level::Low);
    ran.result.unwrap();
    assert!(arms.is_empty(), "{}", ran.out);
    assert!(log.contains("not armed: real check not recorded"), "{log}");
}

// --- The toast ---

#[test]
fn a_low_risk_pr_held_for_its_real_check_notifies_once_per_head() {
    let f = fixture();
    // #3's PR is seen before #4, before #5 and at the stop.
    let gh = github(&[3, 4, 5], &[]);
    let script = Script::new(&f.base(), &gh, Some(needs_check()));
    let notifier = FakeNotifier::default();

    run_loop(&f, &script, &gh, &notifier).result.unwrap();

    let shown: Vec<Notification> = notifier
        .shown()
        .into_iter()
        .filter(|n| n.title.starts_with("PR #103 "))
        .collect();
    assert_eq!(titles(&shown), ["PR #103 awaits your merge (real check)"]);
    assert_eq!(shown[0].key, format!("pr:103:{HEAD}"));
    assert_eq!(shown[0].click, "https://github.com/o/r/pull/103");
    assert!(shown[0].body.contains(CHECK), "{}", shown[0].body);
    assert!(
        shown[0]
            .body
            .contains("next: run it, then sbxm task real-check --pr 103 --pass \"...\""),
        "{}",
        shown[0].body
    );
}

#[test]
fn a_low_risk_pr_whose_check_failed_notifies_too() {
    let f = fixture();
    let gh = github(&[3], &[]);
    post(&gh, HEAD, StatusState::Failure);
    let script = Script::new(&f.base(), &gh, Some(needs_check()));
    let notifier = FakeNotifier::default();

    run_loop(&f, &script, &gh, &notifier).result.unwrap();

    assert_eq!(
        titles(&notifier.shown()),
        ["PR #103 awaits your merge (real check)"]
    );
}

#[test]
fn a_low_risk_pr_with_no_required_check_stays_quiet() {
    let f = fixture();
    let gh = github(&[3], &[]);
    let script = Script::new(&f.base(), &gh, Some("Do it.\n".to_owned()));
    let notifier = FakeNotifier::default();

    run_loop(&f, &script, &gh, &notifier).result.unwrap();

    assert!(notifier.shown().is_empty());
}

#[test]
fn a_medium_risk_pr_with_a_real_check_keeps_the_medium_toast() {
    let f = fixture();
    let gh = github(&[3], &[]);
    let script = Script::new(&f.base(), &gh, Some(needs_check())).risk(Level::Medium);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &script, &gh, &notifier);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert_eq!(
        titles(&notifier.shown()),
        ["PR #103 awaits your merge (medium)"]
    );
}
