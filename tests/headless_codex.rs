use std::path::Path;
use std::time::Duration;

use sbxm::backend::{ExecOutput, FakeBackend, Stdin};
use sbxm::harness::Harness;
use sbxm::headless::{self, HeadlessOpts, RunStatus, Usage};

/// Real `codex exec --json` output (codex-cli in the `codex` sandbox agent,
/// gpt-5.6-luna, 2026-09-29).
const PONG: &str = include_str!("../src/fixtures/codex-ndjson-pong.jsonl");
/// A run with a command and a file change, so several item types appear.
const TOOL: &str = include_str!("../src/fixtures/codex-ndjson-tool.jsonl");
/// An unsupported model: `turn.failed`, exit code 1.
const FAILED: &str = include_str!("../src/fixtures/codex-ndjson-failed.jsonl");
/// The tool run cut off mid-line, as a `timeout` KILL can leave it.
const TRUNCATED: &str = include_str!("../src/fixtures/codex-ndjson-truncated.jsonl");
const TERM_MARKER: &str = "timeout: sending signal TERM to command 'codex'\n";

fn opts(is_git_repo: bool) -> HeadlessOpts {
    HeadlessOpts {
        model: Some("gpt-5.6-luna".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo,
    }
}

fn exec_output(stdout: &str, stderr: &str, exit_code: i32) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(exit_code),
    }
}

fn run_with(output: ExecOutput) -> (FakeBackend, headless::HeadlessResult) {
    let fake = FakeBackend::default().with_exec_outputs(vec![output]);
    let result = headless::run(
        &fake,
        "sb",
        Path::new("/work"),
        Harness::Codex,
        "Reply with exactly: PONG",
        &opts(false),
        Duration::from_secs(600),
    )
    .unwrap();
    (fake, result)
}

#[test]
fn argv_skips_the_git_check_only_outside_a_repo() {
    let outside = Harness::Codex.headless_argv("do it", &opts(false)).unwrap();
    insta::assert_debug_snapshot!(outside, @r#"
    [
        "codex",
        "exec",
        "-m",
        "gpt-5.6-luna",
        "--json",
        "--skip-git-repo-check",
        "do it",
    ]
    "#);
    let inside = Harness::Codex.headless_argv("do it", &opts(true)).unwrap();
    assert!(!inside.contains(&"--skip-git-repo-check".to_owned()));
    assert_eq!(inside.last().unwrap(), "do it");
}

#[test]
fn git_repo_workaround_is_codex_only() {
    assert_eq!(
        Harness::Codex.git_repo_workaround(false),
        Some("--skip-git-repo-check")
    );
    assert_eq!(Harness::Codex.git_repo_workaround(true), None);
    assert_eq!(Harness::Claude.git_repo_workaround(false), None);
}

#[test]
fn stdin_is_an_empty_pipe_not_closed() {
    // `codex exec` blocks on a stdin that is attached but never closed (S5).
    assert_eq!(Harness::Codex.stdin(), Stdin::Piped(String::new()));
    assert_eq!(Harness::Claude.stdin(), Stdin::Closed);
}

#[test]
fn codex_has_no_budget_flag() {
    assert_eq!(Harness::Codex.budget_flag(Some(2.0)), None);
    let with_budget = HeadlessOpts {
        budget_usd: Some(2.0),
        ..opts(false)
    };
    let argv = Harness::Codex.headless_argv("p", &with_budget).unwrap();
    assert!(!argv.iter().any(|a| a.contains("budget")));
}

#[test]
fn parses_a_completed_run() {
    let result = Harness::Codex.parse_headless_output(PONG).unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, PONG);
    assert_eq!(
        result.usage,
        Usage {
            input_tokens: 8142,
            output_tokens: 6,
            cost_usd: None,
        }
    );
}

#[test]
fn the_answer_is_the_last_agent_message() {
    let result = Harness::Codex.parse_headless_output(TOOL).unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "DONE");
    assert_eq!(result.usage.input_tokens, 25206);
    assert_eq!(result.usage.output_tokens, 377);
}

#[test]
fn a_failed_turn_is_failed_with_codexs_message() {
    let result = Harness::Codex.parse_headless_output(FAILED).unwrap();
    let RunStatus::Failed(message) = &result.status else {
        panic!("{result:?}");
    };
    assert!(
        message.contains("'no-such-model' model is not supported"),
        "{message}"
    );
    // The `error` *item* is only a warning and is not an answer.
    assert_eq!(result.answer, "");
}

#[test]
fn a_truncated_run_keeps_the_partial_answer() {
    let result = Harness::Codex.parse_headless_output(TRUNCATED).unwrap();
    assert!(matches!(result.status, RunStatus::Failed(_)), "{result:?}");
    assert!(
        result.answer.starts_with("I’ll create the file"),
        "{result:?}"
    );
    assert_eq!(result.transcript, TRUNCATED);
    assert_eq!(result.usage.input_tokens, 0);
}

#[test]
fn run_pipes_an_empty_stdin_and_wraps_the_command_in_a_timeout() {
    let (fake, result) = run_with(exec_output(
        PONG,
        "Reading additional input from stdin...\n",
        0,
    ));
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "PONG");
    let calls = fake.execs();
    let (sandbox, spec) = &calls[0];
    assert_eq!(sandbox, "sb");
    insta::assert_debug_snapshot!(spec, @r#"
    ExecSpec {
        workdir: Some(
            "/work",
        ),
        argv: [
            "timeout",
            "-v",
            "--kill-after=10",
            "600",
            "codex",
            "exec",
            "-m",
            "gpt-5.6-luna",
            "--json",
            "--skip-git-repo-check",
            "Reply with exactly: PONG",
        ],
        stdin: Piped(
            "",
        ),
    }
    "#);
}

#[test]
fn timeout_with_truncated_ndjson_is_timed_out_with_the_partial_answer() {
    let (_, result) = run_with(exec_output(TRUNCATED, TERM_MARKER, 124));
    assert_eq!(result.status, RunStatus::TimedOut);
    assert!(
        result.answer.starts_with("I’ll create the file"),
        "{result:?}"
    );
}

#[test]
fn a_failed_exit_keeps_the_parsed_message_not_stderr_noise() {
    let (_, result) = run_with(exec_output(
        FAILED,
        "Reading additional input from stdin...\n",
        1,
    ));
    let RunStatus::Failed(message) = &result.status else {
        panic!("{result:?}");
    };
    assert!(
        message.contains("'no-such-model' model is not supported"),
        "{message}"
    );
}

#[test]
fn argv_omits_the_model_flag_when_absent() {
    let no_model = HeadlessOpts {
        model: None,
        ..opts(true)
    };
    let argv = Harness::Codex.headless_argv("do it", &no_model).unwrap();
    insta::assert_debug_snapshot!(argv, @r#"
    [
        "codex",
        "exec",
        "--json",
        "do it",
    ]
    "#);
}

#[test]
fn high_effort_sets_reasoning_effort() {
    let high = HeadlessOpts {
        high_effort: true,
        ..opts(true)
    };
    let argv = Harness::Codex.headless_argv("do it", &high).unwrap();
    insta::assert_debug_snapshot!(argv, @r#"
    [
        "codex",
        "exec",
        "-m",
        "gpt-5.6-luna",
        "-c",
        "model_reasoning_effort=high",
        "--json",
        "do it",
    ]
    "#);
    let normal = Harness::Codex.headless_argv("do it", &opts(true)).unwrap();
    assert!(!normal.iter().any(|a| a.contains("reasoning")));
}
