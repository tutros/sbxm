//! Issue 118: `sbxm task resume (--issue N | --spec FILE) [--rounds N]` as a command.

mod common;

use assert_cmd::Command;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks,
    ready_spec_task, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_resume::{Options, Target, run};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, ProcessProbe, Stage, Status};

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(0)
    }
}

fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn options(f: &Fixture, rounds: Option<u32>) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Issue(41),
        rounds,
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
}

fn go(f: &Fixture, opts: &Options, backend: &FakeBackend, probe: &dyn ProcessProbe) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        opts,
        backend,
        probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

fn leave_in(f: &Fixture, stage: Stage, status: Status) {
    let first = backend(f, &[CLEAN]);
    let mut prepared = worked_task(f, &first);
    prepared.record.stage = stage;
    prepared.record.status = status;
    record::write(&prepared.meta, &prepared.record).unwrap();
}

#[test]
fn a_failed_worker_is_resumed_and_the_output_says_from_where_and_how_it_ended() {
    let f = config();
    leave_in(&f, Stage::Working, Status::Failed);
    let backend = backend(&f, &[CLEAN]);

    let out = go(&f, &options(&f, None), &backend, &Gone);

    out.result.unwrap();
    assert!(
        out.out.contains("issue-41: resuming from working (failed)"),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("issue-41: worker completed"),
        "{}",
        out.out
    );
    assert!(
        out.out
            .contains("issue-41: review round 1: 0 must-fix finding(s)"),
        "{}",
        out.out
    );
    assert!(out.out.contains("issue-41: ready"), "{}", out.out);
    assert!(
        out.out.contains("next: sbxm task finish --issue 41"),
        "{}",
        out.out
    );
}

#[test]
fn rounds_says_how_many_rounds_the_task_now_has_and_a_new_stop_names_rounds_again() {
    let f = config();
    // One round: round 1 finds one, the fix round runs, round 2 still finds one: stopped.
    let first = backend(&f, &[ONE, ONE]);
    let mut prepared = worked_task(&f, &first);
    let env = sbxm::task::pipeline::TaskEnv {
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &first,
        probe: &Gone,
        host: &FakeHostRunner::default(),
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
    let backend = backend(&f, &[ONE]);

    let out = go(&f, &options(&f, Some(1)), &backend, &Gone);

    out.result.unwrap();
    assert!(
        out.out.contains("issue-41: resuming from ready (ok)"),
        "{}",
        out.out
    );
    assert!(
        out.out
            .contains("issue-41: 1 more fix round(s), 2 in all (stopped: rounds-exhausted)"),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("(stopped: rounds-exhausted)"),
        "{}",
        out.out
    );
    assert!(
        out.out
            .contains("or give it more fix rounds: sbxm task resume --issue 41 --rounds 1"),
        "{}",
        out.out
    );
}

#[test]
fn a_live_process_is_refused_with_its_pid() {
    let f = config();
    leave_in(&f, Stage::Working, Status::Running);
    let backend = backend(&f, &[CLEAN]);

    let out = go(&f, &options(&f, None), &backend, &Alive);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("still running"), "{message}");
    assert!(backend.execs().is_empty());
}

#[test]
fn a_missing_task_is_refused() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);

    let out = go(&f, &options(&f, None), &backend, &Gone);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("no task issue-41"), "{message}");
}

#[test]
fn issue_zero_is_refused() {
    let f = config();
    let backend = backend(&f, &[CLEAN]);
    let opts = Options {
        target: Target::Issue(0),
        ..options(&f, None)
    };

    let out = go(&f, &opts, &backend, &Gone);

    assert!(format!("{:#}", out.result.unwrap_err()).contains("start at 1"));
}

#[test]
fn the_cli_has_resume_with_issue_spec_and_rounds() {
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "resume", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in ["--issue", "--spec", "--rounds"] {
        assert!(help.contains(flag), "{help}");
    }
}

#[test]
fn the_cli_needs_a_target() {
    Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "resume"])
        .assert()
        .failure();
}

#[test]
fn a_spec_task_is_resumed_by_its_file() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    );
    let (spec, id) = ready_spec_task(&f);
    // Its review failed (left as a killed or failed reviewer would leave it).
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = record::read(&meta.join("task.json")).unwrap();
    (task.stage, task.status) = (Stage::Reviewing, Status::Failed);
    record::write(&meta, &task).unwrap();
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN.to_owned()]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(reviewer);
    let opts = Options {
        target: Target::Spec(spec),
        ..options(&f, None)
    };

    let out = go(&f, &opts, &backend, &Gone);

    out.result.unwrap();
    assert!(
        out.out
            .contains(&format!("{id}: resuming from reviewing (failed)")),
        "{}",
        out.out
    );
    assert!(out.out.contains(&format!("{id}: ready")), "{}", out.out);
    assert!(
        out.out.contains("next: sbxm task finish --spec"),
        "{}",
        out.out
    );
    let task = record::read(&meta.join("task.json")).unwrap();
    assert_eq!((task.stage, task.status), (Stage::Ready, Status::Ok));
}

/// Issue 41 reviewed through `[ONE, ONE]` with its one fix round: stopped `rounds-exhausted`.
fn stopped(f: &Fixture) {
    let first = backend(f, &[ONE, ONE]);
    let mut prepared = worked_task(f, &first);
    let env = sbxm::task::pipeline::TaskEnv {
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &first,
        probe: &Gone,
        host: &FakeHostRunner::default(),
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
}

#[test]
fn rounds_that_would_overflow_the_budget_are_refused_before_anything_changes() {
    let f = config();
    stopped(&f);
    let path = record::task_dir(&f.env.base_dir(), "issue-41").join("task.json");
    let before = std::fs::read(&path).unwrap();
    let backend = backend(&f, &[CLEAN]);

    let out = go(&f, &options(&f, Some(u32::MAX)), &backend, &Gone);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--rounds 4294967295"), "{message}");
    assert!(message.contains("too many"), "{message}");
    assert!(!message.contains('\n'), "one line: {message}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "task.json unchanged");
    assert!(backend.execs().is_empty());
    assert!(backend.creates().is_empty());
}

#[test]
fn the_largest_rounds_that_fit_are_added_exactly() {
    let f = config();
    stopped(&f);
    let backend = backend(&f, &[CLEAN]);

    let out = go(&f, &options(&f, Some(u32::MAX - 1)), &backend, &Gone);

    out.result.unwrap();
    assert!(
        out.out
            .contains("issue-41: 4294967294 more fix round(s), 4294967295 in all"),
        "{}",
        out.out
    );
    let saved =
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap();
    assert_eq!(saved.fix_rounds, u32::MAX);
}
