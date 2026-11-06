//! Issue 432 (decision 298): before each new issue, `sbxm task loop` resolves the open PRs it
//! opened that GitHub reports conflicting with the base, one at a time, with `task fix --pr N`;
//! auto-merge is disarmed first and armed again only after a clean low-risk resolve whose review
//! covered the PR's new head. The phases are scripted; the PRs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, Mergeable, PrInfo, PrState};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
/// The PRs' heads before a resolve.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// A PR's head after a resolve pushed onto it.
const PUSHED: &str = "1111111111111111111111111111111111111111";
const OTHER: &str = "2222222222222222222222222222222222222222";
/// `main`'s tips.
const MAIN_1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MAIN_2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// What a scripted `task fix --pr N` does.
#[derive(Clone, Copy)]
enum Fix {
    /// Pushes [`PUSHED`]; its review covered `commit` with this risk and must-fix count. With
    /// `conflicting`, GitHub still reports the PR conflicting afterwards.
    Pushed {
        risk: Level,
        must_fix: u32,
        commit: &'static str,
        conflicting: bool,
    },
    /// Fails before any push (a worker that handed back, failing gates).
    Fails(&'static str),
}

const CLEAN: Fix = Fix::Pushed {
    risk: Level::Low,
    must_fix: 0,
    commit: PUSHED,
    conflicting: false,
};

/// Plays `task run`, `task finish`, `task fix --pr N` and `task rm --merged`.
struct Script<'a> {
    base: PathBuf,
    gh: &'a FakeGitHub,
    fixes: HashMap<u32, Fix>,
    /// What the next resolves do, in order, before `fixes` is asked.
    queued: RefCell<Vec<Fix>>,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    /// Run after each `task run`: GitHub changing meanwhile.
    after_run: Box<dyn Fn(&FakeGitHub) + 'a>,
    /// The answers of the disk check, in order; room once they run out.
    room: RefCell<Vec<bool>>,
}

impl<'a> Script<'a> {
    fn new(base: &Path, gh: &'a FakeGitHub) -> Self {
        Self {
            base: base.to_owned(),
            gh,
            fixes: HashMap::new(),
            queued: RefCell::default(),
            calls: RefCell::default(),
            clock: Cell::new(START),
            after_run: Box::new(|_| {}),
            room: RefCell::default(),
        }
    }

    fn fix(mut self, pr: u32, fix: Fix) -> Self {
        self.fixes.insert(pr, fix);
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn fixes_run(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("fix") || c.starts_with("run"))
            .collect()
    }
}

/// A task record of `kind` in `base`, at `stage`, with a review when `review` is given.
fn write_task(
    base: &Path,
    kind: Kind,
    number: u32,
    id: Option<&str>,
    review: Option<(Level, u32, &str)>,
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
    rec.review = review.map(|(risk, must_fix, commit)| ReviewResult {
        round: 1,
        commit: Some(commit.to_owned()),
        must_fix,
        risk,
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
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
            Some((Level::Medium, 0, HEAD)),
        );
        (self.after_run)(self.gh);
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

    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let armed = self.gh.pr_merge_state(REPO, pr)?.auto_merge;
        self.calls
            .borrow_mut()
            .push(format!("fix {pr}{}", if armed { " (armed)" } else { "" }));
        self.clock.set(self.clock.get() + 200);
        let head = self.gh.pr(REPO, pr)?.head_oid;
        let id = record::fix_id(pr, &head).unwrap();
        let queued = {
            let mut queued = self.queued.borrow_mut();
            (!queued.is_empty()).then(|| queued.remove(0))
        };
        match queued.unwrap_or_else(|| self.fixes.get(&pr).copied().unwrap_or(CLEAN)) {
            Fix::Pushed {
                risk,
                must_fix,
                commit,
                conflicting,
            } => {
                // `task fix` finishes the fix task with its push.
                let (dir, mut rec) = write_task(
                    &self.base,
                    Kind::Fix,
                    pr,
                    Some(&id),
                    Some((risk, must_fix, commit)),
                );
                rec.stage = Stage::Finished;
                record::write(&dir, &rec).unwrap();
                self.gh.set_pr_head(pr, PUSHED);
                if !conflicting {
                    self.gh.set_mergeable(pr, Mergeable::Mergeable);
                }
                Ok(())
            }
            Fix::Fails(why) => {
                write_task(&self.base, Kind::Fix, pr, Some(&id), None);
                bail!("{why}")
            }
        }
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
        Ok(())
    }

    fn room(&self) -> Result<()> {
        let mut room = self.room.borrow_mut();
        if room.is_empty() || room.remove(0) {
            Ok(())
        } else {
            Err(anyhow!("only 1 GiB free"))
        }
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

    fn opts(&self, cap: Option<u32>) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap,
            repo: Some(REPO.into()),
        }
    }

    /// A task of issue `issue` a loop finished into PR `100 + issue`, its review as given.
    fn loop_pr(&self, issue: u32, risk: Level) {
        let (dir, mut rec) = write_task(
            &self.base(),
            Kind::Issue,
            issue,
            None,
            Some((risk, 0, HEAD)),
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

/// GitHub with open issues `issues` (each finished into PR `100 + issue`) and the PRs `prs`.
fn github(issues: &[u32], prs: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN_1);
    let mut open = Vec::new();
    for &n in issues {
        gh = gh.with_issue_text(issue_text(n)).with_pr(pr(100 + n, HEAD));
        open.push(open_issue(n, &[READY, "size:S"], ""));
    }
    for &n in prs {
        gh = gh.with_pr(pr(n, HEAD));
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

/// The loop's lines about PRs have no run of spaces, as a message wrapped badly in the source
/// would.
fn assert_one_space(text: &str) {
    for line in text.lines().filter(|l| l.contains("PR #")) {
        assert!(!line.contains("  "), "{line}");
    }
}

fn arms(gh: &FakeGitHub) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrArmAutoMerge(..)))
        .collect()
}

fn disarms(gh: &FakeGitHub) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrDisarmAutoMerge(..)))
        .collect()
}

// --- Which PRs are resolved ---

#[test]
fn a_pr_that_is_only_behind_is_not_resolved() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    // `base_moved` is set for a PR that is only behind; GitHub says it still merges.
    let mut behind = pr(101, HEAD);
    behind.base_moved = true;
    let gh = github(&[], &[]).with_pr(behind);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(script.fixes_run().is_empty(), "{:?}", script.calls());
}

#[test]
fn an_unknown_pr_is_not_resolved_this_step_and_is_read_again_next_step() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101]).with_mergeable(101, Mergeable::Unknown);
    let mut script = Script::new(&f.base(), &gh);
    // GitHub has computed it by the next step: conflicting.
    script.after_run = Box::new(|gh| gh.set_mergeable(101, Mergeable::Conflicting));

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["run 3", "fix 101"], "{}", run.out);
}

#[test]
fn a_conflicting_draft_pr_is_skipped_and_said_once() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_draft(101);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["run 3"]);
    assert_eq!(run.out.matches("PR #101").count(), 1, "{}", run.out);
    assert!(run.out.contains("draft"), "{}", run.out);
    assert_one_space(&run.out);
}

#[test]
fn a_pr_the_loop_did_not_open_is_left_alone() {
    let f = fixture();
    // A task that continued an open PR only pushed to it.
    f.loop_pr(1, Level::Low);
    let dir = record::task_dir(&f.base(), "issue-1");
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.continues = Some(record::PrBranch {
        pr: 101,
        branch: "b101".into(),
        base: HEAD.into(),
    });
    record::write(&dir, &rec).unwrap();
    // And one a command run by hand finished, without the loop's mark.
    let (dir, mut rec) = write_task(&f.base(), Kind::Issue, 2, None, Some((Level::Low, 0, HEAD)));
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/102"));
    record::write(&dir, &rec).unwrap();
    let gh = github(&[], &[101, 102])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_mergeable(102, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert!(script.fixes_run().is_empty(), "{:?}", script.calls());
}

// --- Auto-merge around a resolve ---

#[test]
fn an_armed_conflicting_pr_is_disarmed_before_the_fix_and_armed_again_on_its_new_head() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    // The fix ran with auto-merge already off.
    assert_eq!(script.fixes_run(), ["fix 101"]);
    assert_eq!(disarms(&gh), [GhCall::PrDisarmAutoMerge(REPO.into(), 101)]);
    assert_eq!(
        arms(&gh),
        [GhCall::PrArmAutoMerge(REPO.into(), 101, PUSHED.into())]
    );
    assert!(run.out.contains("armed"), "{}", run.out);
}

#[test]
fn an_unarmed_low_risk_loop_pr_is_armed_after_a_clean_low_risk_resolve() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert!(disarms(&gh).is_empty());
    assert_eq!(
        arms(&gh),
        [GhCall::PrArmAutoMerge(REPO.into(), 101, PUSHED.into())]
    );
}

#[test]
fn an_unarmed_pr_whose_own_review_was_medium_is_not_armed_by_a_low_risk_resolve() {
    let f = fixture();
    f.loop_pr(1, Level::Medium);
    let gh = github(&[], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["fix 101"]);
    assert!(arms(&gh).is_empty(), "{}", run.out);
}

#[test]
fn a_conflicting_medium_or_high_risk_pr_is_resolved_and_not_armed() {
    for risk in [Level::Medium, Level::High] {
        let f = fixture();
        f.loop_pr(1, Level::Low);
        let gh = github(&[], &[101])
            .with_mergeable(101, Mergeable::Conflicting)
            .with_auto_merge(101);
        let script = Script::new(&f.base(), &gh).fix(
            101,
            Fix::Pushed {
                risk,
                must_fix: 0,
                commit: PUSHED,
                conflicting: false,
            },
        );

        let run = run_loop(&f, &f.opts(None), &script, &gh);

        run.result.unwrap();
        assert_eq!(script.fixes_run(), ["fix 101"], "{risk:?}");
        assert_eq!(disarms(&gh).len(), 1, "{risk:?}");
        assert!(arms(&gh).is_empty(), "{risk:?}: {}", run.out);
        assert!(run.out.contains("awaits you"), "{}", run.out);
        assert!(!gh.pr_merge_state(REPO, 101).unwrap().auto_merge);
    }
}

#[test]
fn a_fix_review_that_did_not_cover_the_new_head_is_not_armed_and_says_why() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101);
    let script = Script::new(&f.base(), &gh).fix(
        101,
        Fix::Pushed {
            risk: Level::Low,
            must_fix: 0,
            commit: OTHER,
            conflicting: false,
        },
    );

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(run.out.contains("not armed"), "{}", run.out);
    assert_one_space(&run.out);
    assert!(f.log_text().contains("not armed"), "{}", f.log_text());
}

#[test]
fn a_failed_disarm_leaves_the_pr_unresolved_with_a_warning() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101)
        .failing_disarm("HTTP 502");
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert!(script.fixes_run().is_empty());
    assert!(run.warn.contains("HTTP 502"), "{}", run.warn);
    assert_one_space(&run.warn);
    assert!(arms(&gh).is_empty());
}

#[test]
fn a_low_disk_leaves_a_conflicting_pr_for_a_later_step_not_failed() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);
    // Low at the first step's resolve; room again by the next step.
    script.room.borrow_mut().push(false);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["run 3", "fix 101"], "{}", run.out);
    assert!(!run.out.contains("PR #101 awaits you"), "{}", run.out);
}

#[test]
fn an_earlier_medium_or_high_risk_resolve_keeps_the_pr_unarmed_through_a_later_low_risk_one() {
    for risk in [Level::Medium, Level::High] {
        let f = fixture();
        f.loop_pr(1, Level::Low);
        let gh = github(&[3], &[101])
            .with_mergeable(101, Mergeable::Conflicting)
            .with_auto_merge(101);
        let mut script = Script::new(&f.base(), &gh);
        script.queued.borrow_mut().extend([
            Fix::Pushed {
                risk,
                must_fix: 0,
                commit: PUSHED,
                conflicting: false,
            },
            CLEAN,
        ]);
        // `main` moves and the PR conflicts again by the next step.
        script.after_run = Box::new(|gh| {
            gh.set_branch_head("main", MAIN_2);
            gh.set_mergeable(101, Mergeable::Conflicting);
        });

        let run = run_loop(&f, &f.opts(None), &script, &gh);

        run.result.unwrap();
        assert_eq!(
            script.fixes_run(),
            ["fix 101", "run 3", "fix 101"],
            "{risk:?}: {}",
            run.out
        );
        assert!(arms(&gh).is_empty(), "{risk:?}: {}", run.out);
    }
}

#[test]
fn an_earlier_risky_resolve_keeps_the_pr_unarmed_after_a_loop_restart() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101);
    let first = Script::new(&f.base(), &gh).fix(
        101,
        Fix::Pushed {
            risk: Level::High,
            must_fix: 0,
            commit: PUSHED,
            conflicting: false,
        },
    );
    run_loop(&f, &f.opts(None), &first, &gh).result.unwrap();
    // A new loop, after `main` moved and the PR conflicts again.
    gh.set_branch_head("main", MAIN_2);
    gh.set_mergeable(101, Mergeable::Conflicting);
    let second = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &second, &gh);

    run.result.unwrap();
    assert_eq!(second.fixes_run(), ["fix 101"], "{}", run.out);
    assert!(arms(&gh).is_empty(), "{}", run.out);
}

#[test]
fn earlier_clean_low_risk_resolves_still_let_a_pr_be_armed_again() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101])
        .with_mergeable(101, Mergeable::Conflicting)
        .with_auto_merge(101);
    let mut script = Script::new(&f.base(), &gh);
    script.after_run = Box::new(|gh| {
        gh.set_branch_head("main", MAIN_2);
        gh.set_mergeable(101, Mergeable::Conflicting);
    });

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert_eq!(script.fixes_run(), ["fix 101", "run 3", "fix 101"]);
    assert_eq!(arms(&gh).len(), 2);
}

// --- A failed resolve ---

#[test]
fn a_failed_resolve_leaves_the_pr_and_its_fix_task_is_said_once_and_not_tried_again() {
    let cases = [
        (
            Fix::Pushed {
                risk: Level::Low,
                must_fix: 1,
                commit: PUSHED,
                conflicting: true,
            },
            "must-fix",
        ),
        (
            Fix::Fails("gates failed for fix-101-0123456"),
            "gates failed",
        ),
        (
            Fix::Fails("the worker of fix-101-0123456 made no commits"),
            "no commits",
        ),
    ];
    for (fix, said) in cases {
        let f = fixture();
        f.loop_pr(1, Level::Low);
        let gh = github(&[3, 4], &[101])
            .with_mergeable(101, Mergeable::Conflicting)
            .with_auto_merge(101);
        let mut script = Script::new(&f.base(), &gh).fix(101, fix);
        // `main` moves after each task, so only the failure stops another try.
        let tips = Mutex::new(vec![MAIN_2, OTHER]);
        script.after_run = Box::new(move |gh| {
            if let Some(tip) = tips.lock().unwrap().pop() {
                gh.set_branch_head("main", tip);
            }
        });

        let run = run_loop(&f, &f.opts(None), &script, &gh);

        run.result.unwrap();
        assert_eq!(script.fixes_run(), ["fix 101", "run 3", "run 4"], "{said}");
        assert!(run.out.contains(said), "{said}: {}", run.out);
        assert_eq!(
            run.out.matches("PR #101 awaits you").count(),
            1,
            "{said}: {}",
            run.out
        );
        assert!(arms(&gh).is_empty(), "{said}");
        assert_one_space(&run.out);
        // The fix task stays for the user.
        let id = record::fix_id(101, HEAD).unwrap();
        assert!(record::task_dir(&f.base(), &id).join("task.json").is_file());
        // And the PR counts among those awaiting the user: disarmed, open.
        assert!(!gh.pr_merge_state(REPO, 101).unwrap().auto_merge);
        assert!(f.log_text().contains("#101"), "{}", f.log_text());
    }
}

// --- At most one resolve per PR per `main` tip ---

#[test]
fn a_pr_still_conflicting_with_main_unchanged_is_not_tried_again() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh).fix(
        101,
        Fix::Pushed {
            risk: Level::Low,
            must_fix: 0,
            commit: PUSHED,
            conflicting: true,
        },
    );

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert_eq!(script.fixes_run(), ["fix 101", "run 3"]);
}

#[test]
fn a_pr_conflicting_again_after_main_moved_is_resolved_again() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let mut script = Script::new(&f.base(), &gh);
    script.after_run = Box::new(|gh| {
        gh.set_branch_head("main", MAIN_2);
        gh.set_mergeable(101, Mergeable::Conflicting);
        // The first resolve's fix task is the user's to remove; the second has a new head.
        gh.set_pr_head(101, OTHER);
    });

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(
        script.fixes_run(),
        ["fix 101", "run 3", "fix 101"],
        "{}",
        run.out
    );
    // Each resolve has its own row.
    assert_eq!(
        f.log_text().matches("| #101 ").count(),
        2,
        "{}",
        f.log_text()
    );
}

#[test]
fn two_conflicting_prs_are_resolved_one_after_the_other_in_pr_number_order() {
    let f = fixture();
    // `issue-12` sorts before `issue-5` in a folder listing.
    f.loop_pr(12, Level::Low);
    f.loop_pr(5, Level::Low);
    let gh = github(&[], &[105, 112])
        .with_mergeable(105, Mergeable::Conflicting)
        .with_mergeable(112, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    assert_eq!(script.fixes_run(), ["fix 105", "fix 112"]);
}

// --- The cap ---

#[test]
fn with_five_prs_awaiting_the_user_a_conflicting_one_is_resolved_before_the_pause() {
    let f = fixture();
    let mut gh = github(&[3], &[]);
    for issue in 1..=5 {
        f.loop_pr(issue, Level::Medium);
        gh = gh.with_pr(pr(100 + issue, HEAD));
    }
    let gh = gh.with_mergeable(102, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(None), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["fix 102"], "{}", run.out);
    // Still five await the user (a medium PR isn't armed), so the loop paused before #3.
    assert!(run.sleeps > 0);
    assert!(run.out.contains("5 PRs await you"), "{}", run.out);
}

#[test]
fn a_resolve_does_not_count_toward_the_cap() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[3], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    let run = run_loop(&f, &f.opts(Some(1)), &script, &gh);

    run.result.unwrap();
    assert_eq!(script.fixes_run(), ["fix 101", "run 3"], "{}", run.out);
}

#[test]
fn a_resolve_gets_its_own_row_in_the_loop_log() {
    let f = fixture();
    f.loop_pr(1, Level::Low);
    let gh = github(&[], &[101]).with_mergeable(101, Mergeable::Conflicting);
    let script = Script::new(&f.base(), &gh);

    run_loop(&f, &f.opts(None), &script, &gh).result.unwrap();

    let log = f.log_text();
    let row = log
        .lines()
        .find(|l| l.contains("| #101 "))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(row.contains("resolve"), "{row}");
    assert!(row.contains("auto-merge armed"), "{row}");
}

#[test]
fn the_real_fix_step_runs_task_fix_on_that_pr() {
    let f = fixture();
    let gh = github(&[], &[101]);
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
    };

    let e = steps
        .fix_pr(101, &mut Vec::new(), &mut Vec::new())
        .unwrap_err();

    // `task fix --pr 101` refused it: there is no sbxm review on the PR to start from.
    assert!(format!("{e:#}").contains("PR #101"), "{e:#}");
    assert!(gh.calls().contains(&GhCall::Pr(REPO.into(), 101)));
}

// --- The GitHub calls ---

/// Plays `gh`: records every call and answers with the queued outputs in order.
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

fn gh_answering(outputs: &[&str]) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn a_pr_merge_state_is_conflicting_only_for_conflicting_or_dirty() {
    // The shape of `gh pr view --json state,autoMergeRequest,isDraft,mergeable,mergeStateStatus`
    // (gh 2.83 field names; written by hand, no GitHub access where issue 432 was built).
    let cases = [
        ("CONFLICTING", "DIRTY", Mergeable::Conflicting),
        ("CONFLICTING", "UNKNOWN", Mergeable::Conflicting),
        ("MERGEABLE", "DIRTY", Mergeable::Conflicting),
        ("MERGEABLE", "BEHIND", Mergeable::Mergeable),
        ("MERGEABLE", "CLEAN", Mergeable::Mergeable),
        ("UNKNOWN", "UNKNOWN", Mergeable::Unknown),
    ];
    for (mergeable, status, want) in cases {
        let json = format!(
            r#"{{"autoMergeRequest":null,"isDraft":false,"mergeable":"{mergeable}","mergeStateStatus":"{status}","state":"OPEN"}}"#
        );
        let (gh, _) = gh_answering(&[&json]);
        assert_eq!(
            gh.pr_merge_state(REPO, 7).unwrap().mergeable,
            want,
            "{mergeable} {status}"
        );
    }
    let (gh, _) = gh_answering(&[
        r#"{"autoMergeRequest":null,"isDraft":true,"mergeable":"CONFLICTING","mergeStateStatus":"DIRTY","state":"OPEN"}"#,
    ]);
    assert!(gh.pr_merge_state(REPO, 7).unwrap().draft);
}

#[test]
fn auto_merge_is_disarmed_with_disable_auto() {
    let (gh, script) = gh_answering(&[""]);

    gh.pr_disarm_auto_merge(REPO, 7).unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0],
        ["pr", "merge", "7", "--repo", REPO, "--disable-auto"]
    );
}

#[test]
fn a_branch_head_is_read_from_the_branches_api() {
    // The shape of `gh api repos/<repo>/branches/<branch>`, trimmed to the field read.
    let (gh, script) = gh_answering(&[&format!(
        r#"{{"name":"main","commit":{{"sha":"{MAIN_1}","url":"u"}},"protected":false}}"#
    )]);

    assert_eq!(gh.branch_head(REPO, "main").unwrap(), MAIN_1);
    assert_eq!(
        script.calls.lock().unwrap()[0],
        ["api", "repos/o/r/branches/main"]
    );
}

#[test]
fn a_branch_head_that_is_not_a_full_commit_or_a_bad_branch_name_is_refused() {
    let (gh, _) = gh_answering(&[r#"{"commit":{"sha":"abc"}}"#]);
    assert!(gh.branch_head(REPO, "main").is_err());
    let (gh, script) = gh_answering(&[]);
    assert!(gh.branch_head(REPO, "../x").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}
