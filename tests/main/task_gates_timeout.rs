//! Issue 471: a gate that hits its timeout keeps the end of its output in `gates.log`, under its
//! `failed: …, timed out` line: the same tail a failing gate keeps, `(no output before the
//! timeout)` when it printed nothing, and secret-looking values masked. Sandbox and host tiers
//! alike.

use crate::common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, backend, ctx, ctx_with_host, fixture_with, issue_text, ok,
    play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Tiers};
use sbxm::task::record;

const NO_OUTPUT: &str = "(no output before the timeout)";

fn config(sandbox: &str, host: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [{sandbox}]\nhost = [{host}]\ntimeout = \"3m\"\n"
    ))
}

fn playing(f: &Fixture) -> FakeBackend {
    let what = Play {
        commits: vec!["a.txt".into()],
        result_md: Some(b"done\n".to_vec()),
        bundle_bytes: None,
    };
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what))
}

/// What the sandbox exec of a gate returns when the in-sandbox `timeout -v` killed it.
fn killed(stdout: &str) -> ExecOutput {
    ExecOutput {
        stdout: stdout.to_owned(),
        stderr: "timeout: sending signal TERM to command 'sh'\n".into(),
        exit_code: Some(124),
    }
}

/// Runs the worker for issue 41, then the gates of `tiers`, and returns `gates.log`.
fn gates_log_after(
    f: &Fixture,
    backend: &FakeBackend,
    host: &FakeHostRunner,
    tiers: Tiers,
) -> String {
    let github = FakeGitHub::default();
    let source = source(f);
    let mut prepared =
        pipeline::prepare(&ctx(f, &source, backend, &github), &issue_text(41)).unwrap();
    pipeline::run_worker(&ctx(f, &source, backend, &github), &mut prepared).unwrap();
    let ctx = ctx_with_host(f, &source, backend, &github, host);
    pipeline::run_gates(&ctx.env(), &mut prepared, "after-worker", tiers).unwrap();
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    fs::read_to_string(meta.join("gates.log")).unwrap()
}

/// The block under the status line of the gate entry for `command`.
fn block_of<'a>(log: &'a str, command: &str) -> (&'a str, &'a str) {
    let start = log
        .find(&format!(": {command}\n"))
        .unwrap_or_else(|| panic!("no entry for {command}: {log}"));
    let entry = &log[start..];
    let entry = &entry[entry.find('\n').unwrap() + 1..];
    let (status, rest) = entry.split_once('\n').unwrap();
    let end = rest.find("\n\n").unwrap_or(rest.len());
    (status, &rest[..end])
}

#[test]
fn a_gate_that_ends_just_under_its_timeout_keeps_its_output_as_today() {
    let f = config("\"run-gate\"", "");
    let backend = playing(&f).with_exec_output_matching(
        "run-gate",
        ExecOutput {
            stdout: "line one\nline two\n".into(),
            stderr: String::new(),
            exit_code: Some(1),
        },
    );

    let log = gates_log_after(&f, &backend, &FakeHostRunner::default(), Tiers::ALL);

    let (status, block) = block_of(&log, "run-gate");
    assert_eq!(status, "failed: exit 1");
    assert_eq!(block, "line one\nline two");
}

#[test]
fn a_sandbox_gate_killed_at_its_timeout_keeps_its_last_lines_under_the_timeout_line() {
    let f = config("\"slow-gate\"", "");
    let backend =
        playing(&f).with_exec_output_matching("slow-gate", killed("compiling a\ncompiling b\n"));

    let log = gates_log_after(&f, &backend, &FakeHostRunner::default(), Tiers::ALL);

    let (status, block) = block_of(&log, "slow-gate");
    assert_eq!(status, "failed: exit 124, timed out");
    assert_eq!(block, "compiling a\ncompiling b");
}

#[test]
fn a_sandbox_gate_killed_having_printed_nothing_says_so() {
    let f = config("\"slow-gate\"", "");
    let backend = playing(&f).with_exec_output_matching("slow-gate", killed(""));

    let log = gates_log_after(&f, &backend, &FakeHostRunner::default(), Tiers::ALL);

    let (status, block) = block_of(&log, "slow-gate");
    assert_eq!(status, "failed: exit 124, timed out");
    assert_eq!(block, NO_OUTPUT);
}

/// More output than the tail before the timeout: only its end is kept, exactly what a gate that
/// failed with the same output keeps.
#[test]
fn a_gate_printing_more_than_the_tail_keeps_the_same_tail_as_a_failure() {
    let mut long = "FIRST LINE\n".to_owned();
    for i in 0..2000 {
        long.push_str(&format!("output line {i}\n"));
    }
    long.push_str("LAST LINE\n");
    let f = config("\"slow-gate\", \"fail-gate\"", "");
    let backend = playing(&f)
        .with_exec_output_matching("slow-gate", killed(&long))
        .with_exec_output_matching(
            "fail-gate",
            ExecOutput {
                stdout: long.clone(),
                stderr: String::new(),
                exit_code: Some(1),
            },
        );
    let timed_out_log = gates_log_after(&f, &backend, &FakeHostRunner::default(), Tiers::ALL);
    let g = config("\"fail-gate\"", "");
    let backend = playing(&g).with_exec_output_matching(
        "fail-gate",
        ExecOutput {
            stdout: long.clone(),
            stderr: String::new(),
            exit_code: Some(1),
        },
    );
    let failed_log = gates_log_after(&g, &backend, &FakeHostRunner::default(), Tiers::ALL);

    let (_, timed_out) = block_of(&timed_out_log, "slow-gate");
    let (_, failed) = block_of(&failed_log, "fail-gate");
    assert!(timed_out.ends_with("LAST LINE"), "{timed_out}");
    assert!(!timed_out.contains("FIRST LINE"));
    assert!(timed_out.len() <= 8 * 1024, "{}", timed_out.len());
    // The same tail, less the room `timeout -v`'s own line took in it.
    assert!(failed.ends_with(timed_out), "{timed_out}\n----\n{failed}");
    assert!(failed.len() - timed_out.len() < 100);
}

#[test]
fn a_host_gate_killed_at_its_timeout_keeps_its_last_lines_under_the_timeout_line() {
    let f = config("\"cargo test\"", "\"slow-host\"");
    let backend = playing(&f);
    let host =
        FakeHostRunner::default().with_timeout_after("slow-host", "building\n", "warning: slow\n");

    let log = gates_log_after(&f, &backend, &host, Tiers::ALL);

    let (status, block) = block_of(&log, "slow-host");
    assert_eq!(status, "failed: no exit code, timed out");
    assert_eq!(block, "building\nwarning: slow");
}

#[test]
fn a_host_gate_killed_having_printed_nothing_says_so() {
    let f = config("\"cargo test\"", "\"slow-host\"");
    let backend = playing(&f);
    let host = FakeHostRunner::default().with_timeout("slow-host");

    let log = gates_log_after(&f, &backend, &host, Tiers::ALL);

    let (status, block) = block_of(&log, "slow-host");
    assert_eq!(status, "failed: no exit code, timed out");
    assert_eq!(block, NO_OUTPUT);
}

#[test]
fn secret_looking_values_in_a_timed_out_gates_output_are_masked() {
    let f = config("\"slow-gate\"", "");
    let token = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0");
    let backend =
        playing(&f).with_exec_output_matching("slow-gate", killed(&format!("token is {token}\n")));

    let log = gates_log_after(&f, &backend, &FakeHostRunner::default(), Tiers::ALL);

    assert!(!log.contains(&token), "{log}");
    let (_, block) = block_of(&log, "slow-gate");
    assert_eq!(block, "token is [redacted]");
}
