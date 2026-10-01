//! M2b slice 7, step 3: the gating stage (spec §5.1 step 5, §7): gates run after the worker,
//! results go to `task.json` and `gates.log`, a failure is `gates-failed`, and the host tier
//! only runs on a clean checkout after the sandbox tier passed.

mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, backend, ctx, ctx_with_host, fixture_with, issue_text, ok,
    play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Tiers};
use sbxm::task::record::{self, Stage, Status};

fn config(sandbox: &str, host: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [{sandbox}]\nhost = [{host}]\ntimeout = \"3m\"\n"
    ))
}

fn what() -> Play {
    Play {
        commits: vec!["a.txt".into()],
        result_md: Some(b"done\n".to_vec()),
        bundle_bytes: None,
    }
}

fn playing(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what()))
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

/// Prepares and runs the worker for issue 41, ready for gates.
fn worked(f: &Fixture, backend: &FakeBackend) -> pipeline::Prepared {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx(f, &source, backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &issue_text(41)).unwrap();
    pipeline::run_worker(&ctx, &mut prepared).unwrap();
    prepared
}

fn gates_log(f: &Fixture) -> String {
    fs::read_to_string(meta(f).join("gates.log")).unwrap_or_default()
}

fn run_gates(
    f: &Fixture,
    backend: &FakeBackend,
    host: &FakeHostRunner,
    prepared: &mut pipeline::Prepared,
    phase: &str,
    tiers: Tiers,
) -> pipeline::Gated {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx_with_host(f, &source, backend, &github, host);
    pipeline::run_gates(&ctx.gate_env(), prepared, phase, tiers).unwrap()
}

#[test]
fn passing_sandbox_gates_move_the_task_to_gating_passed_and_are_recorded() {
    let f = config("\"cargo test\", \"cargo fmt --check\"", "");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);
    let host = FakeHostRunner::default();

    let gated = run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(gated.passed && gated.failed.is_none());
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert_eq!(record.gates.len(), 2);
    assert_eq!(
        (
            record.gates[0].phase.as_str(),
            record.gates[0].tier.as_str(),
            record.gates[0].command.as_str()
        ),
        ("after-worker", "sandbox", "cargo test")
    );
    assert!(record.gates.iter().all(|g| g.passed && g.exit == Some(0)));
    assert_eq!(prepared.record, record);
    let log = gates_log(&f);
    assert!(
        log.contains("cargo test") && log.contains("passed"),
        "{log}"
    );
    assert!(host.calls().is_empty(), "no host tier configured");
}

#[test]
fn the_first_failing_gate_is_gates_failed_and_stops_the_rest() {
    let f = config(
        "\"cargo fmt --check\", \"cargo clippy\", \"cargo test\"",
        "\"cargo build\"",
    );
    let backend = playing(&f).with_exec_output_matching(
        "clippy",
        ExecOutput {
            stdout: String::new(),
            stderr: "error: lint failed\n".into(),
            exit_code: Some(101),
        },
    );
    let mut prepared = worked(&f, &backend);
    let host = FakeHostRunner::default();

    let gated = run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(!gated.passed);
    let failed = gated.failed.unwrap();
    assert_eq!(
        (failed.command.as_str(), failed.exit),
        ("cargo clippy", Some(101))
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::GatesFailed)
    );
    assert_eq!(record.gates.len(), 2, "cargo test must not have run");
    assert!(
        host.calls().is_empty(),
        "the host tier only runs after the sandbox tier passed"
    );
    let log = gates_log(&f);
    assert!(
        log.contains("lint failed") && log.contains("failed"),
        "{log}"
    );
}

#[test]
fn the_gating_stage_is_on_disk_before_any_gate_runs() {
    let f = config("\"cargo test\"", "");
    let seen = Arc::new(Mutex::new(None));
    let seen_in_hook = Arc::clone(&seen);
    let record_path = meta(&f).join("task.json");
    let player = play_tasks(&f.env.base_dir(), "main", what());
    let backend = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if spec.argv.iter().any(|a| a == "cargo test") {
                let record = record::read(&record_path).unwrap();
                *seen_in_hook.lock().unwrap() = Some((record.stage, record.status));
            }
            player(sandbox, spec);
        });
    let mut prepared = worked(&f, &backend);

    run_gates(
        &f,
        &backend,
        &FakeHostRunner::default(),
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert_eq!(
        *seen.lock().unwrap(),
        Some((Stage::Gating, Status::Running))
    );
}

#[test]
fn host_gates_run_in_a_clean_checkout_of_the_committed_branch_which_is_removed_after() {
    let f = config("\"cargo test\"", "\"cargo build\", \"cargo doc\"");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);
    let checkout = f.env.base_dir().join("tasks").join("issue-41-gates");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_hook = Arc::clone(&seen);
    let host = FakeHostRunner::default().with_hook(move |cwd, command| {
        seen_in_hook.lock().unwrap().push((
            cwd.to_path_buf(),
            command.to_owned(),
            cwd.join("a.txt").is_file(),
            cwd.join(".sbxm-task").exists(),
        ));
    });

    let gated = run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(gated.passed);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (cwd, _, has_commit, has_agent_dir) in seen.iter() {
        assert_eq!(cwd, &checkout);
        assert!(*has_commit, "the committed file must be in the checkout");
        assert!(!has_agent_dir, "sbxm's own folder must not");
    }
    assert!(
        !checkout.exists(),
        "the checkout is removed after the gates"
    );
    let record = saved(&f);
    assert_eq!(record.gates.iter().filter(|g| g.tier == "host").count(), 2);
    assert_eq!(host.calls()[0].timeout.as_secs(), 180);
}

#[test]
fn a_gates_log_that_cannot_be_written_does_not_strand_the_task_in_running() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);
    // A folder where the log file should be: every write to it fails.
    fs::create_dir_all(meta(&f).join("gates.log")).unwrap();

    let gated = run_gates(
        &f,
        &backend,
        &FakeHostRunner::default(),
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(gated.passed);
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert!(
        record.notes.iter().any(|n| n.contains("gates.log")),
        "{:?}",
        record.notes
    );
}

#[test]
fn a_leftover_checkout_from_an_earlier_run_is_cleared_not_reported_as_a_gate_failure() {
    let f = config("\"cargo test\"", "\"cargo build\"");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);
    // What a killed sbxm (or a build that still held the folder) leaves behind.
    let checkout = f.env.base_dir().join("tasks").join("issue-41-gates");
    fs::create_dir_all(checkout.join("target")).unwrap();
    fs::write(checkout.join("stale.txt"), "from an earlier run\n").unwrap();
    let saw_stale = Arc::new(Mutex::new(None));
    let saw_stale_in_hook = Arc::clone(&saw_stale);
    let host = FakeHostRunner::default().with_hook(move |cwd, _| {
        *saw_stale_in_hook.lock().unwrap() = Some(cwd.join("stale.txt").exists());
    });

    let gated = run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(gated.passed, "{:?}", gated.failed);
    assert_eq!(
        *saw_stale.lock().unwrap(),
        Some(false),
        "the host gate ran in a clean checkout"
    );
    assert!(!checkout.exists());
}

#[test]
fn a_failing_host_gate_is_gates_failed_and_the_checkout_is_still_removed() {
    let f = config("\"cargo test\"", "\"cargo build\"");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);
    let host = FakeHostRunner::default().with_exit("build", 1, "", "link error\n");

    let gated = run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(!gated.passed);
    assert_eq!(gated.failed.unwrap().tier, "host");
    assert_eq!(saved(&f).status, Status::GatesFailed);
    assert!(
        !f.env
            .base_dir()
            .join("tasks")
            .join("issue-41-gates")
            .exists()
    );
    assert!(gates_log(&f).contains("link error"));
}

#[test]
fn a_tier_can_be_chosen_and_a_failed_task_can_be_gated_again() {
    let f = config("\"cargo test\"", "\"cargo build\"");
    let backend = playing(&f).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(1),
        },
    );
    let mut prepared = worked(&f, &backend);
    let host = FakeHostRunner::default();
    run_gates(
        &f,
        &backend,
        &host,
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );
    assert_eq!(saved(&f).status, Status::GatesFailed);

    // Only the host tier this time (as `task gates --tier host` would): it does not need the
    // sandbox tier to pass first, because the user asked for it alone.
    let gated = run_gates(&f, &backend, &host, &mut prepared, "on-demand", Tiers::HOST);

    assert!(gated.passed);
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert_eq!(
        record.gates.len(),
        2,
        "the earlier failure stays in the history"
    );
    assert_eq!(record.gates[1].phase, "on-demand");
    assert_eq!(host.calls().len(), 1);
}

#[test]
fn no_gates_configured_passes_and_says_so_in_the_log() {
    let f = config("", "");
    let backend = playing(&f);
    let mut prepared = worked(&f, &backend);

    let gated = run_gates(
        &f,
        &backend,
        &FakeHostRunner::default(),
        &mut prepared,
        "after-worker",
        Tiers::ALL,
    );

    assert!(gated.passed);
    assert_eq!(saved(&f).status, Status::Passed);
    assert!(gates_log(&f).contains("no gates"), "{}", gates_log(&f));
}

// ---- Gates after the worker in `start` ----

fn github_with(numbers: &[u32]) -> FakeGitHub {
    numbers.iter().fold(FakeGitHub::default(), |gh, n| {
        gh.with_issue_text(issue_text(*n))
    })
}

#[test]
fn start_gates_the_worker_and_reports_it() {
    let f = config("\"cargo test\"", "");
    let (backend, github) = (playing(&f), github_with(&[41]));
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[41]);

    let gates = reports[0].gates.as_ref().unwrap().as_ref().unwrap();
    assert!(gates.passed);
    assert_eq!(saved(&f).status, Status::Passed);
}

#[test]
fn start_reports_failed_gates_without_hiding_the_worker() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(1),
        },
    );
    let github = github_with(&[41]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[41]);

    assert!(reports[0].result.is_ok());
    assert!(!reports[0].gates.as_ref().unwrap().as_ref().unwrap().passed);
    assert_eq!(saved(&f).status, Status::GatesFailed);
}

#[test]
fn a_failed_worker_is_not_gated() {
    let f = config("\"cargo test\"", "");
    let backend = backend()
        .with_exec_output_matching(
            "claude",
            ExecOutput {
                stdout: String::new(),
                stderr: "boom".into(),
                exit_code: Some(1),
            },
        )
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what()));
    let github = github_with(&[41]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[41]);

    assert!(reports[0].gates.is_none());
    assert_eq!(saved(&f).stage, Stage::Working);
    assert!(
        !backend
            .execs()
            .iter()
            .any(|(_, spec)| spec.argv.iter().any(|a| a == "cargo test"))
    );
}

#[test]
fn a_timed_out_worker_is_still_gated() {
    let f = config("\"cargo test\"", "");
    let backend = backend()
        .with_exec_output_matching(
            "claude",
            ExecOutput {
                stdout: String::new(),
                stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
                exit_code: Some(124),
            },
        )
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what()));
    let github = github_with(&[41]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[41]);

    assert!(reports[0].gates.as_ref().unwrap().as_ref().unwrap().passed);
}
