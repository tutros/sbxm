//! Issue 510 (decision 306): with `[loop] should_fix = "fix"`, a clean task whose review has
//! should-fix findings sized S gets one more fix round on them (`task resume --rounds 1
//! --should-fix`) before `task finish`; the rest are filed as today. The phases are scripted.

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
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::Stopped;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;

const CLEAN: &str = "Must-fix findings: 0\n";
const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";
const S_AND_M: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n\n\
    ### S-2 - b.txt:3 a bigger thing.\n\n**Where:** b.txt:3\n**Size:** M (a module)\n";
const NOT_S: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a bigger thing.\n\n**Where:** a.txt:2\n**Size:** L (a format)\n\n\
    ### S-2 - b.txt:3 no size.\n\n**Where:** b.txt:3\n";

/// What the should-fix round's review ends with.
#[derive(Clone, Copy)]
struct After {
    review: &'static str,
    must_fix: u32,
}

/// Plays `task run` (a clean review with `review`), the should-fix round, `task finish` and `task
/// file-findings`.
struct Script {
    base: PathBuf,
    review: &'static str,
    after: After,
    /// The record says the task already had its should-fix round.
    had_round: bool,
    /// How many should-fix steps fail first, refused before they write anything.
    should_fix_fails: Cell<u32>,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
}

impl Script {
    fn new(base: &Path, review: &'static str) -> Self {
        Self {
            base: base.to_owned(),
            review,
            after: After {
                review: CLEAN,
                must_fix: 0,
            },
            had_round: false,
            should_fix_fails: Cell::new(0),
            calls: RefCell::default(),
            clock: Cell::new(START),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn dir(&self, issue: u32) -> PathBuf {
        record::task_dir(&self.base, &format!("issue-{issue}"))
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        self.clock.set(self.clock.get() + 300);
        let dir = self.dir(issue);
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
        rec.should_fix_round = self.had_round;
        rec.review = Some(ReviewResult {
            round: 1,
            must_fix: 0,
            ..ReviewResult::default()
        });
        record::write(&dir, &rec).unwrap();
        fs::write(dir.join("review.md"), self.review).unwrap();
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn should_fix(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("should-fix {issue}"));
        if self.should_fix_fails.get() > 0 {
            self.should_fix_fails.set(self.should_fix_fails.get() - 1);
            return Err(anyhow!("refused before any write"));
        }
        let dir = self.dir(issue);
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.should_fix_round = true;
        rec.round += 1;
        rec.fix_rounds = rec.round;
        rec.review = Some(ReviewResult {
            round: 3,
            must_fix: self.after.must_fix,
            ..ReviewResult::default()
        });
        if self.after.must_fix > 0 {
            rec.stopped = Some(Stopped::RoundsExhausted);
        }
        record::write(&dir, &rec).unwrap();
        fs::write(dir.join("review.md"), self.after.review).unwrap();
        Ok(())
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("finish {issue}"));
        let dir = self.dir(issue);
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec).unwrap();
        Ok(())
    }

    fn file_findings(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let review = fs::read_to_string(self.dir(issue).join("review.md")).unwrap();
        let ids: Vec<&str> = ["S-1", "S-2"]
            .into_iter()
            .filter(|id| review.contains(id))
            .collect();
        self.calls
            .borrow_mut()
            .push(format!("file-findings {issue} [{}]", ids.join(" ")));
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

/// `should_fix`: the `[loop] should_fix` line, or none.
fn fixture(should_fix: Option<&str>) -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    let table =
        should_fix.map_or_else(String::new, |v| format!("\n[loop]\nshould_fix = \"{v}\"\n"));
    fs::write(
        repo_root.join("sbxm-task.toml"),
        format!("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n{table}"),
    )
    .unwrap();
    Fixture { env, repo_root }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(issue_text(7))
        .with_open_issues(vec![open_issue(7, &[READY, "size:S"], "")])
}

fn run_loop(f: &Fixture, script: &Script) -> (Result<()>, String) {
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let stop = || false;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let opts = Options {
        repo_root: f.repo_root.clone(),
        issues: Vec::new(),
        cap: Some(1),
        repo: Some(REPO.into()),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        &opts,
        script,
        &github(),
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn should_fix_unset_or_file_files_the_s_finding_with_no_extra_round() {
    for setting in [None, Some("file")] {
        let f = fixture(setting);
        let script = Script::new(&f.env.base_dir(), S_ONE);

        let (result, out) = run_loop(&f, &script);

        result.unwrap();
        assert_eq!(
            script.calls(),
            ["run 7", "finish 7", "file-findings 7 [S-1]"],
            "{setting:?}: {out}"
        );
    }
}

#[test]
fn one_s_should_fix_gets_one_round_then_finish_and_nothing_left_to_file() {
    let f = fixture(Some("fix"));
    let script = Script::new(&f.env.base_dir(), S_ONE);

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "should-fix 7", "finish 7", "file-findings 7 []"],
        "{out}"
    );
}

#[test]
fn with_an_s_and_an_m_the_round_runs_and_the_m_is_filed() {
    let f = fixture(Some("fix"));
    let mut script = Script::new(&f.env.base_dir(), S_AND_M);
    // The full review after the round still finds the M one.
    script.after.review = "Must-fix findings: 0\n\n## Should fix\n\n\
        ### S-2 - b.txt:3 a bigger thing.\n\n**Where:** b.txt:3\n**Size:** M (a module)\n";

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "should-fix 7", "finish 7", "file-findings 7 [S-2]"],
        "{out}"
    );
}

#[test]
fn only_m_l_or_unsized_should_fix_gets_no_round_and_is_filed() {
    let f = fixture(Some("fix"));
    let script = Script::new(&f.env.base_dir(), NOT_S);

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "finish 7", "file-findings 7 [S-1 S-2]"],
        "{out}"
    );
}

#[test]
fn an_s_finding_the_round_left_open_is_filed_and_the_task_finishes() {
    let f = fixture(Some("fix"));
    let mut script = Script::new(&f.env.base_dir(), S_ONE);
    script.after.review = S_ONE;

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "should-fix 7", "finish 7", "file-findings 7 [S-1]"],
        "only one round, then filed: {out}"
    );
}

#[test]
fn a_new_must_fix_from_the_round_is_a_work_stop_with_nothing_filed() {
    let f = fixture(Some("fix"));
    let mut script = Script::new(&f.env.base_dir(), S_ONE);
    script.after = After {
        review: "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n",
        must_fix: 1,
    };

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "should-fix 7", "finish 7"],
        "{out}"
    );
    let log = fs::read_dir(f.env.base_dir().join(".sbxm/loop-logs"))
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(
        log.contains("draft PR #107, work stop: rounds-exhausted"),
        "{log}"
    );
}

#[test]
fn a_task_that_already_had_its_should_fix_round_never_gets_a_second() {
    let f = fixture(Some("fix"));
    let mut script = Script::new(&f.env.base_dir(), S_ONE);
    script.had_round = true;

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        ["run 7", "finish 7", "file-findings 7 [S-1]"],
        "{out}"
    );
}

#[test]
fn a_should_fix_round_refused_before_any_write_is_tried_once_more_like_any_phase() {
    let f = fixture(Some("fix"));
    let script = Script::new(&f.env.base_dir(), S_ONE);
    script.should_fix_fails.set(1);

    let (result, out) = run_loop(&f, &script);

    result.unwrap();
    assert_eq!(
        script.calls(),
        [
            "run 7",
            "should-fix 7",
            "should-fix 7",
            "finish 7",
            "file-findings 7 []"
        ],
        "{out}"
    );
}
