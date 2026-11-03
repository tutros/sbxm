//! M2b slice 7, step 4: `sbxm task gates --issue N [--tier ..] [--dry-run]` (spec §4, §7).

mod common;

use std::path::PathBuf;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture_with, ok, play_tasks, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_gates::{Options, run};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::Tiers;
use sbxm::task::record::{self, ProcessProbe, Stage, Status};

struct Dead;

impl ProcessProbe for Dead {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn config(sandbox: &str, host: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [{sandbox}]\nhost = [{host}]\ntimeout = \"20m\"\n"
    ))
}

fn playing(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

fn options(f: &Fixture, tiers: Tiers, dry_run: bool) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issue: 41,
        tiers,
        dry_run,
        identity: None,
        disk: sbxm::disk::Guard::default(),
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
}

fn go(
    f: &Fixture,
    opts: &Options,
    backend: &FakeBackend,
    probe: &dyn ProcessProbe,
    host: &FakeHostRunner,
) -> Out {
    let mut out = Vec::new();
    let result = run(&f.env.config_dir(), opts, backend, probe, host, &mut out);
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
}

// ---- --dry-run ----

#[test]
fn a_dry_run_prints_each_tier_and_changes_nothing() {
    let f = config("\"cargo fmt --check\", \"cargo test\"", "");
    let (backend, host) = (backend(), FakeHostRunner::default());

    let out = go(&f, &options(&f, Tiers::ALL, true), &backend, &Probe, &host);

    out.result.unwrap();
    let text = &out.out;
    assert!(
        text.contains("issue-41") && text.contains("dry run"),
        "{text}"
    );
    assert!(
        text.contains("sandbox tier") && text.contains("sbxm-task-issue-41-claude"),
        "{text}"
    );
    assert!(
        text.contains("1. cargo fmt --check") && text.contains("2. cargo test"),
        "{text}"
    );
    assert!(text.contains("20m"), "{text}");
    assert!(text.contains("host tier: off"), "{text}");
    assert!(backend.log().is_empty() && backend.execs().is_empty());
    assert!(host.calls().is_empty());
    assert!(
        !f.env.base_dir().join(".sbxm").exists(),
        "a dry run writes nothing"
    );
}

#[test]
fn a_dry_run_names_the_host_checkout_and_says_it_runs_agent_code_here() {
    let f = config("\"cargo test\"", "\"cargo build\", \"cargo doc\"");

    let out = go(
        &f,
        &options(&f, Tiers::ALL, true),
        &backend(),
        &Probe,
        &FakeHostRunner::default(),
    );

    let text = &out.out;
    assert!(
        text.contains("host tier") && !text.contains("host tier: off"),
        "{text}"
    );
    assert!(
        text.contains("1. cargo build") && text.contains("2. cargo doc"),
        "{text}"
    );
    assert!(text.contains("issue-41-gates"), "{text}");
    assert!(
        text.contains("agent-written code on this machine"),
        "{text}"
    );
}

#[test]
fn a_dry_run_says_an_empty_sandbox_list_means_no_sandbox_gates() {
    let f = config("", "");

    let out = go(
        &f,
        &options(&f, Tiers::ALL, true),
        &backend(),
        &Probe,
        &FakeHostRunner::default(),
    );

    assert!(out.out.contains("no commands"), "{}", out.out);
}

#[test]
fn a_dry_run_with_one_tier_chosen_marks_the_other_as_not_selected() {
    let f = config("\"cargo test\"", "\"cargo build\"");

    let out = go(
        &f,
        &options(&f, Tiers::HOST, true),
        &backend(),
        &Probe,
        &FakeHostRunner::default(),
    );

    assert!(
        out.out.contains("sandbox tier: not selected"),
        "{}",
        out.out
    );
    assert!(out.out.contains("cargo build"), "{}", out.out);
}

// ---- Running ----

#[test]
fn gates_run_on_a_finished_worker_and_the_outcome_is_printed() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let host = FakeHostRunner::default();

    let out = go(&f, &options(&f, Tiers::ALL, false), &backend, &Probe, &host);

    out.result.unwrap();
    assert!(
        out.out.contains("gates passed (1 sandbox, 0 host)"),
        "{}",
        out.out
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert_eq!(record.gates[0].phase, "on-demand");
}

#[test]
fn failing_gates_fail_the_command_naming_the_command_and_the_log() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let failing = backend.with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(101),
        },
    );

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &failing,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("cargo test") && message.contains("exit 101"),
        "{message}"
    );
    assert!(message.contains("gates.log"), "{message}");
    assert_eq!(saved(&f).status, Status::GatesFailed);
}

#[test]
fn only_the_chosen_tier_runs() {
    let f = config("\"cargo test\"", "\"cargo build\"");
    let backend = playing(&f);
    worked_task(&f, &backend);
    // The bundle step runs for every tier (the host tier reads repo.git); gates are the `sh` runs.
    let gate_runs = |b: &FakeBackend| {
        b.execs()
            .iter()
            .filter(|(_, spec)| spec.argv.iter().any(|a| a == "sh"))
            .count()
    };
    let host = FakeHostRunner::default();
    // The host tier builds on a sandbox pass of the same commit, so that comes first.
    go(
        &f,
        &options(&f, Tiers::SANDBOX, false),
        &backend,
        &Probe,
        &host,
    )
    .result
    .unwrap();
    assert!(host.calls().is_empty());
    let before = gate_runs(&backend);

    go(
        &f,
        &options(&f, Tiers::HOST, false),
        &backend,
        &Probe,
        &host,
    )
    .result
    .unwrap();

    assert_eq!(
        gate_runs(&backend),
        before,
        "no sandbox gate command for --tier host"
    );
    assert_eq!(host.calls().len(), 1);
}

#[test]
fn a_task_that_does_not_exist_is_refused_with_how_to_list_them() {
    let f = config("\"cargo test\"", "");
    let backend = backend();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("no task issue-41") && message.contains("sbxm task status"),
        "{message}"
    );
    assert!(backend.execs().is_empty());
}

#[test]
fn a_worker_that_is_still_running_is_refused_and_nothing_runs() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    // Back to "the worker is running" (as it would be during `sbxm task start`).
    prepared.record.stage = Stage::Working;
    prepared.record.status = Status::Running;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let before = backend.execs().len();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("worker still running"), "{message}");
    assert_eq!(backend.execs().len(), before);
}

#[test]
fn an_interrupted_worker_is_refused_with_that_word() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    prepared.record.stage = Stage::Working;
    prepared.record.status = Status::Running;
    record::write(&prepared.meta, &prepared.record).unwrap();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Dead,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("interrupted"), "{message}");
}

#[test]
fn a_failed_worker_has_nothing_to_gate() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    prepared.record.status = Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("failed") && message.contains("nothing to gate"),
        "{message}"
    );
}

#[test]
fn a_task_past_the_gates_is_refused_with_the_stage() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    prepared.record.stage = Stage::Reviewing;
    prepared.record.status = Status::Completed;
    record::write(&prepared.meta, &prepared.record).unwrap();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("reviewing") && message.contains("after the worker"),
        "{message}"
    );
}

#[test]
fn interrupted_gates_can_be_run_again_and_the_interruption_is_noted() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    // sbxm died while the gates were running: Gating/Running with a process that is gone.
    prepared
        .record
        .begin_gating(1, sbxm::task::record::Process::new(1, 1))
        .unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Dead,
        &FakeHostRunner::default(),
    );

    out.result.unwrap();
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert!(
        record.notes.iter().any(|n| n.contains("interrupted")),
        "{:?}",
        record.notes
    );
}

#[test]
fn gates_that_are_really_still_running_are_refused() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    let mut prepared = worked_task(&f, &backend);
    prepared
        .record
        .begin_gating(0, sbxm::task::record::Process::new(1, 0))
        .unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    let before = backend.execs().len();

    // The probe says that process exists and started when the record says it did.
    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("gates are running"), "{message}");
    assert_eq!(backend.execs().len(), before);
}

#[test]
fn gates_can_be_run_again_after_a_failure() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let failing = playing(&f).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(1),
        },
    );
    assert!(
        go(
            &f,
            &options(&f, Tiers::ALL, false),
            &failing,
            &Probe,
            &FakeHostRunner::default()
        )
        .result
        .is_err()
    );

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    out.result.unwrap();
    assert_eq!(saved(&f).status, Status::Passed);
    assert_eq!(saved(&f).gates.len(), 2, "both runs are in the history");
}

#[test]
fn a_missing_config_file_says_how_to_make_one() {
    let f = config("\"cargo test\"", "");
    let mut opts = options(&f, Tiers::ALL, true);
    opts.repo_root = PathBuf::from(f.env.tmp.path()).join("nowhere");

    let out = go(&f, &opts, &backend(), &Probe, &FakeHostRunner::default());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("sbxm-task.toml") && message.contains("sbxm task init"),
        "{message}"
    );
}

// ---- a manual fix after a failed gate ----

#[test]
fn a_commit_made_by_hand_in_the_workers_clone_reaches_repo_git_before_the_gates_pass() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    let repo_git = record::task_dir(&f.env.base_dir(), "issue-41").join("repo.git");
    // The user fixes what the gate found, in the worker's clone.
    std::fs::write(workspace.join("fixed-by-hand.txt"), "fix\n").unwrap();
    common::git(&workspace, &["add", "-A"]);
    common::git(&workspace, &["commit", "-q", "-m", "fix by hand"]);
    let fixed = common::git(&workspace, &["rev-parse", "HEAD"]);
    assert_ne!(common::git(&repo_git, &["rev-parse", "issue-41"]), fixed);

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    out.result.unwrap();
    assert_eq!(
        common::git(&repo_git, &["rev-parse", "issue-41"]),
        fixed,
        "review, the host gates and finish read repo.git, so it must have the fix"
    );
}

// ---- The task's lock (issue 58) ----

#[test]
fn two_gate_runs_released_together_run_once_and_refuse_once() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    // Every exec waits at the gate: a run that got past the checks stops there.
    let (backend, gate) = backend.with_exec_gate();
    let host = FakeHostRunner::default();
    let opts = options(&f, Tiers::ALL, false);
    let barrier = std::sync::Barrier::new(2);
    let (done, results) = std::sync::mpsc::channel();

    let outs: Vec<Out> = std::thread::scope(|s| {
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let done = done.clone();
                let (f, opts, backend, host, barrier) = (&f, &opts, &backend, &host, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    let out = go(f, opts, backend, &Probe, host);
                    done.send(()).unwrap();
                    out
                })
            })
            .collect();
        gate.wait_for_blocked(1);
        // The other run refuses without reaching the gate: two runs at the gate is the race.
        let refused = results.recv_timeout(std::time::Duration::from_secs(10));
        gate.open();
        let outs = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(
            refused.is_ok(),
            "the second gate run got past the checks too"
        );
        outs
    });

    let ran: Vec<&Out> = outs.iter().filter(|o| o.result.is_ok()).collect();
    assert_eq!(ran.len(), 1, "exactly one run");
    let refusal = outs
        .iter()
        .find_map(|o| o.result.as_ref().err())
        .map(|e| format!("{e:#}"))
        .unwrap();
    assert!(
        refusal.contains("task issue-41 is in use by another sbxm command (process "),
        "{refusal}"
    );
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Passed)
    );
    assert_eq!(record.gates.len(), 1, "{:?}", record.gates);
    let log =
        std::fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("gates.log"))
            .unwrap();
    assert_eq!(log.matches("== on-demand").count(), 1, "{log}");
    assert!(
        !record::task_dir(&f.env.base_dir(), "issue-41")
            .join("lock.json")
            .exists(),
        "the lock goes with the run that held it"
    );
}

#[test]
fn a_gate_run_is_refused_while_another_live_command_holds_the_task() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let _held = sbxm::task::lock::ProcessLock::acquire(&meta, "task issue-41", &Probe).unwrap();
    let before = (
        backend.execs().len(),
        std::fs::read(meta.join("task.json")).unwrap(),
    );

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Probe,
        &FakeHostRunner::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("task issue-41 is in use by another sbxm command (process ")
            && message.contains("started "),
        "{message}"
    );
    assert_eq!(
        (
            backend.execs().len(),
            std::fs::read(meta.join("task.json")).unwrap()
        ),
        before,
        "nothing ran and the record is untouched"
    );
}

#[test]
fn a_stale_lock_of_a_gone_command_is_taken_over_and_the_gates_run() {
    let f = config("\"cargo test\"", "");
    let backend = playing(&f);
    worked_task(&f, &backend);
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    std::fs::write(
        meta.join("lock.json"),
        r#"{"pid":4000000,"started_at":"2026-01-01T00:00:00Z","token":"t"}"#,
    )
    .unwrap();

    let out = go(
        &f,
        &options(&f, Tiers::ALL, false),
        &backend,
        &Dead,
        &FakeHostRunner::default(),
    );

    out.result.unwrap();
    assert_eq!(saved(&f).status, Status::Passed);
    assert!(!meta.join("lock.json").exists());
}
