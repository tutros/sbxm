//! Issue 612 (decisions 317(2), 321(2) and (4)): the loop runs each issue task's `task run` and
//! `task resume` as a child `sbxm` and decides from the `outcome.json` the child wrote and the
//! task record, never from error types or output text. The children are played by [`Children`],
//! which writes what a real child would; one test runs the real binary as the child.

mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_loop::child::{Child, ChildSpawner, Exit, Spawner};
use sbxm::commands::task_loop::{self, Env, Options, PhaseEnd, Steps, TaskSteps};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::outcome::{self, Kind, Outcome};
use sbxm::task::record::{self, NewTask, Process, Record, ReviewResult, Stage, Status};

const REPO: &str = "o/r";
const ISSUE: u32 = 3;
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;

/// What a played child does before it exits.
#[derive(Clone)]
enum Act {
    /// Reaches `ready`, clean.
    Ready,
    /// Fails, leaving the record at `working`.
    Fail(&'static str),
    /// Stops on a usage limit of `provider`, leaving the record at `working`.
    UsageLimit(&'static str),
    /// `sbx` signed out, before any record.
    SbxSignedOut,
    /// `sbx` not answering, before any record.
    Environment,
    /// A missing secret, before any record.
    MissingSecret,
    /// Killed after its record was written: no outcome.
    Killed,
    /// Killed before it wrote its record: no record, no outcome.
    KilledEarly,
    /// Refused before any record, saying so in its outcome.
    Refused(&'static str),
    /// Writes an `outcome.json` that can't be read as one.
    Garbage(&'static str),
    /// Refused before any folder, printing this error, and leaving no outcome at all (as a child
    /// that can't read its config does).
    RefusedSilently(&'static str),
}

/// Plays the children in order, writing the record and the outcome a real child would.
struct Children {
    base: PathBuf,
    acts: RefCell<VecDeque<Act>>,
    started: RefCell<Vec<Child>>,
}

impl Children {
    fn new(base: &Path, acts: &[Act]) -> Self {
        Self {
            base: base.to_owned(),
            acts: RefCell::new(acts.iter().cloned().collect()),
            started: RefCell::default(),
        }
    }

    fn args(&self) -> Vec<String> {
        self.started
            .borrow()
            .iter()
            .map(|c| c.args.join(" "))
            .collect()
    }
}

impl Spawner for Children {
    fn run(&self, child: &Child, tick: &dyn Fn()) -> Result<Exit> {
        tick();
        self.started.borrow_mut().push(child.clone());
        let dir = record::task_dir(&self.base, &format!("issue-{ISSUE}"));
        let elsewhere = child
            .env
            .iter()
            .find(|(k, _)| k == outcome::LOOP_FILE_ENV)
            .map(|(_, v)| PathBuf::from(v))
            .expect("the loop names the file for an outcome with no task folder");
        let ended = |o: Outcome| {
            if dir.is_dir() {
                outcome::write(&dir, &o).unwrap();
            } else {
                outcome::write_file(&elsewhere, &o).unwrap();
            }
            Ok(Exit {
                success: o.end == Kind::Ready,
                how: format!("exited with code {}", u8::from(o.end != Kind::Ready)),
                errors: String::new(),
            })
        };
        let act = self.acts.borrow_mut().pop_front().unwrap_or(Act::Ready);
        match act {
            Act::Ready => {
                write_record(&self.base, Stage::Ready);
                ended(Outcome::new(Kind::Ready, None, ""))
            }
            Act::Fail(text) => {
                write_record(&self.base, Stage::Working);
                ended(Outcome::new(Kind::Failed, None, text))
            }
            Act::UsageLimit(provider) => {
                write_record(&self.base, Stage::Working);
                ended(Outcome::new(
                    Kind::UsageLimit,
                    Some(provider.into()),
                    "usage limit reached",
                ))
            }
            Act::SbxSignedOut => ended(Outcome::new(
                Kind::SignedOut,
                Some("sbx".into()),
                "Not authenticated to Docker",
            )),
            Act::Environment => ended(Outcome::new(
                Kind::Environment,
                None,
                "sbx stopped answering",
            )),
            Act::MissingSecret => ended(Outcome::new(
                Kind::MissingSecret,
                None,
                "the sbx secret anthropic is not set; set it with `sbx secret set -g anthropic`",
            )),
            Act::Killed => {
                write_record(&self.base, Stage::Working);
                Ok(Exit {
                    success: false,
                    how: "was killed".into(),
                    errors: String::new(),
                })
            }
            Act::Refused(text) => ended(Outcome::new(Kind::Refused, None, text)),
            Act::KilledEarly => Ok(Exit {
                success: false,
                how: "was killed".into(),
                errors: String::new(),
            }),
            Act::Garbage(text) => {
                write_record(&self.base, Stage::Working);
                fs::write(dir.join("outcome.json"), text).unwrap();
                Ok(Exit {
                    success: false,
                    how: "exited with code 1".into(),
                    errors: String::new(),
                })
            }
            Act::RefusedSilently(error) => Ok(Exit {
                success: false,
                how: "exited with code 1".into(),
                errors: format!("warning: something\nError: {error}\n"),
            }),
        }
    }

    fn stop(&self) {}
}

/// Writes `issue-3`'s record at `stage`.
fn write_record(base: &Path, stage: Stage) {
    let dir = record::task_dir(base, &format!("issue-{ISSUE}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap_or_else(|_| {
        Record::new(
            &NewTask {
                kind: record::Kind::Issue,
                number: ISSUE,
                repo: REPO,
                title: "Fix 3",
                base: "main",
                branch: "sbxm/issue-3",
                config_hash: "h",
                id: None,
            },
            START,
            Process::new(1, 0),
        )
    });
    rec.stage = stage;
    rec.status = if stage == Stage::Working {
        Status::Failed
    } else {
        Status::Ok
    };
    if stage == Stage::Ready {
        rec.review = Some(ReviewResult {
            round: 1,
            must_fix: 0,
            not_run: Some(Vec::new()),
            ..ReviewResult::default()
        });
    }
    record::write(&dir, &rec).unwrap();
}

/// The real [`TaskSteps`] for the task's own phases, the loop's other steps played.
struct Mixed<'a> {
    real: TaskSteps<'a>,
    base: PathBuf,
    calls: RefCell<Vec<String>>,
}

impl Steps for Mixed<'_> {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("run of #{issue} in the loop's process"))
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("resume of #{issue} in the loop's process"))
    }

    fn run_task(
        &self,
        issue: u32,
        location: Option<Location>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> PhaseEnd {
        self.real.run_task(issue, location, out, warn)
    }

    fn resume_task(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> PhaseEnd {
        self.real.resume_task(issue, out, warn)
    }

    fn should_fix_task(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> PhaseEnd {
        self.real.should_fix_task(issue, out, warn)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("finish {issue}"));
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json"))?;
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec)
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
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\nusage_wait = \
         \"30m\"\nusage_wait_total = \"1h\"\n",
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
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_issue_text(issue_text(ISSUE))
        .with_open_issues(vec![open_issue(ISSUE, &["ready-for-worker", "size:S"], "")])
}

struct Run {
    result: Result<()>,
    out: String,
    calls: Vec<String>,
    children: Vec<String>,
}

fn run_loop(f: &Fixture, acts: &[Act]) -> Run {
    let gh = github();
    let backend = FakeBackend::default();
    let host = FakeHostRunner::default();
    let children = Children::new(&f.base(), acts);
    let config_dir = f.env.config_dir();
    let steps = Mixed {
        real: TaskSteps {
            config_dir: &config_dir,
            repo_root: f.repo_root.clone(),
            repo: Some(REPO.into()),
            backend: &backend,
            github: &gh,
            probe: &Probe,
            host: &host,
            disk: sbxm::disk::Guard::default(),
            stop: None,
            spawner: &children,
        },
        base: f.base(),
        calls: RefCell::default(),
    };
    let clock = std::cell::Cell::new(START);
    let env = Env {
        now: &|| clock.get(),
        sleep: &|d: Duration| clock.set(clock.get() + d.as_secs()),
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let result = task_loop::run(
        &config_dir,
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: Some(1),
            repo: Some(REPO.into()),
            wait: false,
        },
        &steps,
        &gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
        calls: steps.calls.take(),
        children: children.args(),
    }
}

const RUN: &str = "task run --issue 3 --repo o/r";
const RESUME: &str = "task resume --issue 3";

#[test]
fn a_child_that_ends_ready_is_finished_as_before() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN]);
    assert_eq!(run.calls, ["finish 3", "file-findings 3"]);
    let log = f.log_text();
    assert!(log.contains("PR #103, clean"), "{log}");
}

#[test]
fn the_window_shows_one_line_per_event_of_the_child() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Ready]);

    let events: Vec<&str> = run
        .out
        .lines()
        .filter(|l| l.starts_with("[issue-3 "))
        .collect();
    assert_eq!(
        events,
        [
            "[issue-3 local] task run started",
            "[issue-3 local] task run ended: ready"
        ],
        "{}",
        run.out
    );
}

#[test]
fn a_failed_child_is_resumed_once_as_a_child_then_parked() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Fail("boom"), Act::Fail("boom again")]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RESUME]);
    assert!(
        run.out.contains(": stopped (boom); resuming it once"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("parked at working after a second failure: boom again")
    );
    assert!(
        f.log_text().contains("parked at working"),
        "{}",
        f.log_text()
    );
}

#[test]
fn a_usage_limit_is_waited_out_then_resumed() {
    let f = fixture();

    let run = run_loop(&f, &[Act::UsageLimit("anthropic"), Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RESUME]);
    assert!(
        run.out.contains(": usage limit; waiting 30m00s"),
        "{}",
        run.out
    );
    assert!(
        f.log_text().contains("usage limit, 30m00s"),
        "{}",
        f.log_text()
    );
}

#[test]
fn sbx_signed_out_stops_the_loop_naming_the_steps_on() {
    let f = fixture();

    let run = run_loop(&f, &[Act::SbxSignedOut]);

    let e = run.result.unwrap_err();
    assert_eq!(
        format!("{e:#}"),
        "the loop stops: sbx is not signed in (Not authenticated to Docker); run `sbx login`, \
         then `sbxm task loop`"
    );
    assert_eq!(task_loop::exit_code(&Err(e)), task_loop::NEEDS_USER);
    assert!(f.log_text().contains("stopped at no record: sign-in"));
}

#[test]
fn a_missing_secret_stops_the_loop_for_the_user() {
    let f = fixture();

    let run = run_loop(&f, &[Act::MissingSecret]);

    let e = run.result.unwrap_err();
    assert!(
        format!("{e:#}").contains("the sbx secret anthropic is not set"),
        "{e:#}"
    );
    assert_eq!(task_loop::exit_code(&Err(e)), task_loop::NEEDS_USER);
    assert_eq!(run.children, [RUN]);
}

#[test]
fn an_environment_stop_before_any_record_is_run_again_once() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Environment, Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RUN]);
    assert_eq!(run.calls, ["finish 3", "file-findings 3"]);
}

#[test]
fn a_killed_child_is_counted_as_interrupted_and_resumed_once() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Killed, Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RESUME]);
    assert!(
        run.out.contains(
            "stopped (the task's child sbxm was killed and wrote no outcome); resuming it once"
        ),
        "{}",
        run.out
    );
    assert!(f.log_text().contains("PR #103, clean"), "{}", f.log_text());
}

#[test]
fn a_killed_child_twice_is_parked() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Killed, Act::Killed]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RESUME]);
    assert!(
        f.log_text().contains("parked at working"),
        "{}",
        f.log_text()
    );
}

#[test]
fn an_outcome_that_does_not_parse_or_names_an_unknown_kind_is_a_failure_quoted() {
    for text in [
        r#"{"end": "ready-ish", "provider": null, "detail": ""}"#,
        "{ not json",
    ] {
        let f = fixture();

        let run = run_loop(&f, &[Act::Garbage(text), Act::Ready]);

        run.result.unwrap();
        assert_eq!(run.children, [RUN, RESUME], "{text}");
        let quoted = format!("{:?}", text.trim());
        assert!(
            run.out.contains(&quoted) && run.out.contains("resuming it once"),
            "{text}: {}",
            run.out
        );
        // Never trusted further: not read as ready, not finished before the resume.
        assert_eq!(run.calls, ["finish 3", "file-findings 3"]);
    }
}

#[test]
fn a_child_that_writes_no_outcome_before_any_record_is_interrupted_quoting_its_error() {
    let f = fixture();

    let run = run_loop(&f, &[Act::RefusedSilently("no such issue"), Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RUN]);
    assert!(
        run.out.contains(
            "stopped (the task's child sbxm exited with code 1 and wrote no outcome (it said: no \
             such issue)); resuming it once"
        ),
        "{}",
        run.out
    );
    assert_eq!(run.calls, ["finish 3", "file-findings 3"]);
}

#[test]
fn a_child_whose_outcome_says_refused_skips_the_issue() {
    let f = fixture();

    let run = run_loop(&f, &[Act::Refused("no such issue")]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN]);
    assert!(
        run.out
            .contains("skipped #3: refused before any write: no such issue"),
        "{}",
        run.out
    );
    assert!(run.calls.is_empty());
}

// --- the real steps around one child ---

fn real_steps<'a>(
    f: &'a Fixture,
    config_dir: &'a Path,
    gh: &'a FakeGitHub,
    backend: &'a FakeBackend,
    host: &'a FakeHostRunner,
    spawner: &'a dyn Spawner,
) -> TaskSteps<'a> {
    TaskSteps {
        config_dir,
        repo_root: f.repo_root.clone(),
        repo: Some(REPO.into()),
        backend,
        github: gh,
        probe: &Probe,
        host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
        spawner,
    }
}

#[test]
fn an_earlier_runs_outcome_is_not_read_as_this_childs() {
    let f = fixture();
    write_record(&f.base(), Stage::Working);
    let dir = record::task_dir(&f.base(), "issue-3");
    outcome::write(&dir, &Outcome::new(Kind::Ready, None, "")).unwrap();
    let elsewhere = outcome::loop_file(&f.base(), "issue-3");
    fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    outcome::write_file(&elsewhere, &Outcome::new(Kind::Ready, None, "")).unwrap();
    let (gh, backend, host) = (github(), FakeBackend::default(), FakeHostRunner::default());
    let children = Children::new(&f.base(), &[Act::Killed]);
    let config_dir = f.env.config_dir();
    let steps = real_steps(&f, &config_dir, &gh, &backend, &host, &children);

    let ended = steps.resume_task(ISSUE, &mut Vec::new(), &mut Vec::new());

    assert!(matches!(ended, PhaseEnd::Missing(_)), "{ended:?}");
}

#[test]
fn the_childs_location_and_its_flags() {
    let f = fixture();
    let (gh, backend, host) = (github(), FakeBackend::default(), FakeHostRunner::default());
    let children = Children::new(&f.base(), &[Act::Ready, Act::Ready, Act::Ready]);
    let config_dir = f.env.config_dir();
    let steps = real_steps(&f, &config_dir, &gh, &backend, &host, &children);
    let mut out = Vec::new();

    steps.run_task(ISSUE, Some(Location::Cloud), &mut out, &mut Vec::new());
    steps.should_fix_task(ISSUE, &mut out, &mut Vec::new());

    assert_eq!(
        children.args(),
        [
            "task run --issue 3 --cloud --repo o/r",
            "task resume --issue 3 --rounds 1 --should-fix"
        ]
    );
    let started = children.started.borrow();
    let config = config_dir.display().to_string();
    for child in started.iter() {
        assert_eq!(child.dir, f.repo_root);
        assert!(
            child
                .env
                .contains(&("SBXM_CONFIG_DIR".to_owned(), config.clone())),
            "{:?}",
            child.env
        );
    }
    let obeys = |c: &Child| {
        c.env
            .iter()
            .any(|(k, _)| k == task_loop::child::STOP_FILE_ENV)
    };
    assert!(
        obeys(&started[0]),
        "a run's slot wait obeys the loop's stop"
    );
    assert!(!obeys(&started[1]), "a should-fix round's doesn't");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("[issue-3 cloud] task run started"), "{out}");
    // The resume reads the task's record for where it runs: here, local.
    assert!(
        out.contains("[issue-3 local] should-fix round ended: ready"),
        "{out}"
    );
}

#[test]
fn a_first_ctrl_c_while_a_child_runs_writes_the_loops_stop_file() {
    struct Pressing<'a>(&'a task_loop::StopFlag);
    impl Spawner for Pressing<'_> {
        fn run(&self, _child: &Child, tick: &dyn Fn()) -> Result<Exit> {
            tick();
            self.0.press();
            tick();
            Ok(Exit {
                success: false,
                how: "exited with code 1".into(),
                errors: String::new(),
            })
        }
        fn stop(&self) {}
    }
    let f = fixture();
    let flag = std::sync::Arc::new(task_loop::StopFlag::default());
    let (gh, backend, host) = (github(), FakeBackend::default(), FakeHostRunner::default());
    let spawner = Pressing(&flag);
    let config_dir = f.env.config_dir();
    let mut steps = real_steps(&f, &config_dir, &gh, &backend, &host, &spawner);
    steps.stop = Some(std::sync::Arc::clone(&flag));
    let stop_file = task_loop::stop_path(&f.base());

    steps.run_task(ISSUE, None, &mut Vec::new(), &mut Vec::new());

    assert!(stop_file.exists());
}

// --- the real binary as the child ---

#[test]
fn the_real_binary_as_a_child_writes_its_outcome_and_logs_its_output_only_to_run_log() {
    let f = fixture();
    // A task recorded, but resumed from a checkout without `sbxm-task.toml`: the child fails.
    write_record(&f.base(), Stage::Working);
    fs::remove_file(f.repo_root.join("sbxm-task.toml")).unwrap();
    let (gh, backend, host) = (github(), FakeBackend::default(), FakeHostRunner::default());
    let spawner = ChildSpawner::new(PathBuf::from(env!("CARGO_BIN_EXE_sbxm")));
    let config_dir = f.env.config_dir();
    let steps = real_steps(&f, &config_dir, &gh, &backend, &host, &spawner);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let ended = steps.resume_task(ISSUE, &mut out, &mut warn);

    let PhaseEnd::Outcome(o) = &ended else {
        panic!("{ended:?}")
    };
    assert_eq!(o.end, Kind::Failed, "{o:?}");
    assert!(o.detail.contains("there is no sbxm-task.toml"), "{o:?}");
    let dir = record::task_dir(&f.base(), "issue-3");
    assert!(dir.join("outcome.json").is_file());
    let run_log = fs::read_to_string(dir.join("run.log")).unwrap();
    assert!(run_log.contains("there is no sbxm-task.toml"), "{run_log}");
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "[issue-3 local] task resume started",
            "[issue-3 local] task resume ended: failed"
        ],
        "{out}"
    );
    assert!(warn.is_empty(), "{}", String::from_utf8_lossy(&warn));
}

#[test]
fn the_real_binary_refused_before_any_folder_writes_its_outcome_where_the_loop_said() {
    let f = fixture();
    let (gh, backend, host) = (github(), FakeBackend::default(), FakeHostRunner::default());
    let spawner = ChildSpawner::new(PathBuf::from(env!("CARGO_BIN_EXE_sbxm")));
    let config_dir = f.env.config_dir();
    let steps = real_steps(&f, &config_dir, &gh, &backend, &host, &spawner);

    let ended = steps.resume_task(ISSUE, &mut Vec::new(), &mut Vec::new());

    // `task resume` of a task that doesn't exist: refused, and no folder made for its outcome.
    let PhaseEnd::Outcome(o) = &ended else {
        panic!("{ended:?}")
    };
    assert_eq!(o.end, Kind::Refused, "{o:?}");
    assert!(!record::task_dir(&f.base(), "issue-3").exists());
    assert!(
        !outcome::loop_file(&f.base(), "issue-3").exists(),
        "read, then removed"
    );
}

/// A child whose own child keeps its error output open after it exits (as a daemon `sbx` starts
/// might) is still seen to have ended.
#[cfg(unix)]
#[test]
fn a_child_is_done_when_it_exits_whatever_keeps_its_output_open() {
    let tmp = tempfile::tempdir().unwrap();
    let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));
    let child = Child {
        args: vec![
            "-c".into(),
            "echo 'Error: gone' >&2; sleep 30 >&2 & exit 3".into(),
        ],
        dir: tmp.path().to_owned(),
        env: Vec::new(),
    };
    let started = std::time::Instant::now();

    let exit = spawner.run(&child, &|| {}).unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(!exit.success);
    assert_eq!(exit.how, "exited with code 3");
    assert!(exit.errors.contains("Error: gone"), "{exit:?}");
}

// --- review of issue 612: a child killed before its record, and secrets in what it says ---

#[test]
fn a_child_killed_before_its_record_is_run_again_once() {
    let f = fixture();

    let run = run_loop(&f, &[Act::KilledEarly, Act::Ready]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RUN], "{}", run.out);
    assert!(!run.out.contains("refused before any write"), "{}", run.out);
    assert_eq!(run.calls, ["finish 3", "file-findings 3"]);
}

#[test]
fn a_child_killed_twice_before_its_record_is_parked() {
    let f = fixture();

    let run = run_loop(&f, &[Act::KilledEarly, Act::KilledEarly]);

    run.result.unwrap();
    assert_eq!(run.children, [RUN, RUN], "{}", run.out);
    assert!(
        f.log_text().contains("parked at no record"),
        "{}",
        f.log_text()
    );
}

/// A GitHub-token-shaped value, made up.
const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123";

#[test]
fn a_credential_in_what_a_child_says_reaches_neither_the_window_nor_the_log() {
    for act in [
        Act::Fail(Box::leak(
            format!("bad remote https://{TOKEN}@github.com/o/r").into_boxed_str(),
        )),
        Act::RefusedSilently(Box::leak(
            format!("bad repo https://{TOKEN}@x").into_boxed_str(),
        )),
        Act::Garbage(Box::leak(format!("{{ not json {TOKEN}").into_boxed_str())),
    ] {
        let f = fixture();

        let run = run_loop(&f, &[act.clone(), act]);

        assert!(!run.out.contains(TOKEN), "{}", run.out);
        assert!(!f.log_text().contains(TOKEN), "{}", f.log_text());
        assert!(run.out.contains("[redacted]"), "{}", run.out);
    }
}
