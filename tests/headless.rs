use std::path::Path;
use std::time::Duration;

use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::harness::Harness;
use sbxm::headless::{self, HeadlessOpts, RunStatus, Usage};

/// Trimmed from real `claude -p ... --output-format stream-json --verbose`
/// output (Claude Code 2.1.280, 2026-09-29).
const PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
/// The same run cut off mid-line, as a `timeout` KILL can leave it.
const TRUNCATED: &str = include_str!("../src/fixtures/claude-stream-json-truncated.jsonl");
/// Real `timeout -v` stderr from `sbx exec` (coreutils in the sandbox).
const TERM_MARKER: &str = "timeout: sending signal TERM to command 'claude'\n";
const KILL_MARKER: &str = "timeout: sending signal TERM to command 'claude'\ntimeout: sending signal KILL to command 'claude'\n";

fn opts() -> HeadlessOpts {
    HeadlessOpts {
        model: Some("claude-haiku-4-5-20251001".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo: false,
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
        Harness::Claude,
        "Reply with exactly: PONG",
        &opts(),
        Duration::from_secs(600),
    )
    .unwrap();
    (fake, result)
}

#[test]
fn claude_argv_and_stdin() {
    let argv = Harness::Claude.headless_argv("do it", &opts()).unwrap();
    insta::assert_debug_snapshot!(argv, @r#"
    [
        "claude",
        "-p",
        "do it",
        "--model",
        "claude-haiku-4-5-20251001",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    "#);
    assert_eq!(Harness::Claude.stdin(), sbxm::backend::Stdin::Closed);
}

#[test]
fn claude_budget_becomes_a_flag() {
    assert_eq!(
        Harness::Claude.budget_flag(Some(2.5)),
        Some(vec!["--max-budget-usd".to_owned(), "2.5".to_owned()])
    );
    assert_eq!(Harness::Claude.budget_flag(None), None);
    let with_budget = HeadlessOpts {
        budget_usd: Some(2.5),
        ..opts()
    };
    let argv = Harness::Claude.headless_argv("p", &with_budget).unwrap();
    assert_eq!(&argv[argv.len() - 2..], ["--max-budget-usd", "2.5"]);
}

#[test]
fn parses_a_completed_stream() {
    let result = Harness::Claude.parse_headless_output(PONG).unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, PONG);
    // input + cache creation + cache read
    assert_eq!(result.usage.input_tokens, 10 + 13952 + 13689);
    assert_eq!(result.usage.output_tokens, 44);
    // Not `assert_eq!`: serde_json's default float parsing can be 1 ulp off.
    let Usage { cost_usd, .. } = result.usage;
    assert!(
        (cost_usd.unwrap() - 0.0295029).abs() < 1e-12,
        "{cost_usd:?}"
    );
}

#[test]
fn parses_a_truncated_stream_best_effort() {
    let result = Harness::Claude.parse_headless_output(TRUNCATED).unwrap();
    assert!(matches!(result.status, RunStatus::Failed(_)));
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, TRUNCATED);
    assert_eq!(result.usage.input_tokens, 10 + 13952 + 13689);
    assert_eq!(result.usage.output_tokens, 6);
    assert_eq!(result.usage.cost_usd, None);
}

#[test]
fn an_error_result_is_failed_with_its_answer_kept() {
    let error = PONG
        .replace("\"is_error\":false", "\"is_error\":true")
        .replace(
            "\"subtype\":\"success\"",
            "\"subtype\":\"error_max_budget_usd\"",
        );
    let result = Harness::Claude.parse_headless_output(&error).unwrap();
    assert_eq!(
        result.status,
        RunStatus::Failed("claude reported error_max_budget_usd".into())
    );
    assert_eq!(result.answer, "PONG");
}

#[test]
fn run_wraps_the_command_in_an_in_sandbox_timeout() {
    let (fake, result) = run_with(exec_output(PONG, "", 0));
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "PONG");

    let calls = fake.execs();
    assert_eq!(calls.len(), 1);
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
            "claude",
            "-p",
            "Reply with exactly: PONG",
            "--model",
            "claude-haiku-4-5-20251001",
            "--output-format",
            "stream-json",
            "--verbose",
        ],
        stdin: Closed,
    }
    "#);
}

#[test]
fn timeout_exit_124_with_marker_is_timed_out_with_the_partial_answer() {
    let (_, result) = run_with(exec_output(TRUNCATED, TERM_MARKER, 124));
    assert_eq!(result.status, RunStatus::TimedOut);
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, TRUNCATED);
}

#[test]
fn timeout_that_needed_a_kill_is_still_timed_out() {
    let (_, result) = run_with(exec_output(TRUNCATED, KILL_MARKER, 137));
    assert_eq!(result.status, RunStatus::TimedOut);
    assert_eq!(result.answer, "PONG");
}

#[test]
fn bare_137_without_the_marker_is_failed_not_timed_out() {
    let (_, result) = run_with(exec_output(TRUNCATED, "Killed\n", 137));
    assert!(matches!(result.status, RunStatus::Failed(_)), "{result:?}");
    assert_eq!(result.answer, "PONG");
}

#[test]
fn a_crash_with_no_output_reports_the_exit_code_and_stderr_not_a_missing_event() {
    let (_, result) = run_with(exec_output("", "boom\n", 1));
    assert_eq!(result.status, RunStatus::Failed("exit code 1: boom".into()));
}

#[test]
fn a_truncated_stream_with_a_bad_exit_reports_the_exit_code() {
    let (_, result) = run_with(exec_output(TRUNCATED, "Killed\n", 137));
    assert_eq!(
        result.status,
        RunStatus::Failed("exit code 137: Killed".into())
    );
    // The partial answer is still kept.
    assert_eq!(result.answer, "PONG");
}

#[test]
fn a_non_zero_exit_overrides_a_completed_stream() {
    let (_, result) = run_with(exec_output(PONG, "boom\n", 1));
    assert_eq!(result.status, RunStatus::Failed("exit code 1: boom".into()));
    assert_eq!(result.answer, "PONG");
}

#[test]
fn a_harness_without_an_adapter_is_refused_before_any_exec() {
    let fake = FakeBackend::default();
    let err = headless::run(
        &fake,
        "sb",
        Path::new("/work"),
        Harness::Gemini,
        "p",
        &opts(),
        Duration::from_secs(60),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "gemini has no headless adapter; use claude, codex or antigravity"
    );
    assert!(fake.execs().is_empty());
}

#[test]
fn claude_omits_the_model_flag_when_absent() {
    let no_model = HeadlessOpts {
        model: None,
        ..opts()
    };
    let argv = Harness::Claude.headless_argv("do it", &no_model).unwrap();
    assert!(!argv.contains(&"--model".to_owned()));
    assert_eq!(argv[..3], ["claude", "-p", "do it"]);
}

#[test]
fn high_effort_changes_nothing_for_claude() {
    let high = HeadlessOpts {
        high_effort: true,
        ..opts()
    };
    assert_eq!(
        Harness::Claude.headless_argv("do it", &high).unwrap(),
        Harness::Claude.headless_argv("do it", &opts()).unwrap()
    );
}
