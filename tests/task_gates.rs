//! M2b slice 7: gates (spec §7, decision 160). Step 1: the sandbox tier runs each command as
//! `sh -c` under the in-sandbox timeout and stops at the first failure.

use std::path::Path;
use std::time::Duration;

use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::task::gates::run_sandbox_tier;

/// What every sandbox gate command starts with: `sbx exec` reads no login profile, so the
/// toolchains the profile installs under `~/.cargo` are put on the PATH here (slice 13, M-1).
const CARGO_ENV: &str = "[ -f \"$HOME/.cargo/env\" ] && . \"$HOME/.cargo/env\"; ";

const SANDBOX: &str = "sbxm-task-issue-41-claude";

fn cmds(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn exit(code: i32, stdout: &str, stderr: &str) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(code),
    }
}

#[test]
fn every_command_runs_as_sh_c_under_the_timeout_in_the_workspace() {
    let backend = FakeBackend::default();

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/e/base/tasks/issue-41"),
        "after-worker",
        &cmds(&["cargo fmt --check", "cargo test"]),
        Duration::from_secs(1200),
    );

    assert_eq!(outcomes.len(), 2);
    let execs = backend.execs();
    assert_eq!(execs.len(), 2);
    for ((sandbox, spec), command) in execs.iter().zip(["cargo fmt --check", "cargo test"]) {
        assert_eq!(sandbox, SANDBOX);
        assert_eq!(
            spec.argv,
            [
                "timeout".to_owned(),
                "-v".to_owned(),
                "--kill-after=10".to_owned(),
                "1200".to_owned(),
                "sh".to_owned(),
                "-c".to_owned(),
                format!("{CARGO_ENV}{command}")
            ]
        );
        assert_eq!(
            spec.workdir.as_deref(),
            Some(Path::new("/e/base/tasks/issue-41"))
        );
    }
}

#[test]
fn passing_gates_are_recorded_with_phase_tier_command_and_exit() {
    let backend = FakeBackend::default();

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/w"),
        "after-fix",
        &cmds(&["cargo test"]),
        Duration::from_secs(60),
    );

    let result = &outcomes[0].result;
    assert_eq!(
        (
            result.phase.as_str(),
            result.tier.as_str(),
            result.command.as_str()
        ),
        ("after-fix", "sandbox", "cargo test")
    );
    assert_eq!((result.exit, result.passed), (Some(0), true));
    assert!(!outcomes[0].timed_out);
}

#[test]
fn the_first_failure_stops_the_tier() {
    let backend = FakeBackend::default()
        .with_exec_output_matching("clippy", exit(101, "", "error: lint failed\n"));

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/w"),
        "after-worker",
        &cmds(&["cargo fmt --check", "cargo clippy", "cargo test"]),
        Duration::from_secs(60),
    );

    assert_eq!(
        outcomes.len(),
        2,
        "cargo test must not run after clippy failed"
    );
    assert!(outcomes[0].result.passed);
    assert!(!outcomes[1].result.passed);
    assert_eq!(outcomes[1].result.exit, Some(101));
    assert!(
        outcomes[1].output_tail.contains("lint failed"),
        "{}",
        outcomes[1].output_tail
    );
    assert_eq!(backend.execs().len(), 2);
}

#[test]
fn a_command_that_hits_the_timeout_is_a_failure_marked_as_timed_out() {
    let backend = FakeBackend::default().with_default_exec_output(exit(
        124,
        "",
        "timeout: sending signal TERM to command 'sh'\n",
    ));

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/w"),
        "after-worker",
        &cmds(&["cargo test"]),
        Duration::from_secs(5),
    );

    assert!(outcomes[0].timed_out);
    assert!(!outcomes[0].result.passed);
}

#[test]
fn a_command_that_cannot_be_run_is_a_failure_with_the_reason() {
    let backend = FakeBackend::default().with_failing_exec_for(SANDBOX);

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/w"),
        "after-worker",
        &cmds(&["cargo test", "cargo fmt --check"]),
        Duration::from_secs(5),
    );

    assert_eq!(
        outcomes.len(),
        1,
        "a gate that could not run also stops the tier"
    );
    assert!(!outcomes[0].result.passed);
    assert_eq!(outcomes[0].result.exit, None);
    assert!(
        outcomes[0].output_tail.contains("fake exec failure"),
        "{}",
        outcomes[0].output_tail
    );
}

#[test]
fn no_commands_means_no_outcomes_and_no_exec() {
    let backend = FakeBackend::default();

    let outcomes = run_sandbox_tier(
        &backend,
        SANDBOX,
        Path::new("/w"),
        "after-worker",
        &[],
        Duration::from_secs(5),
    );

    assert!(outcomes.is_empty());
    assert!(backend.execs().is_empty());
}
