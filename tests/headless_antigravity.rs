use std::path::Path;
use std::time::Duration;

use sbxm::backend::{ExecOutput, FakeBackend, Stdin};
use sbxm::harness::Harness;
use sbxm::headless::{self, HeadlessOpts, RunStatus};

// Real `agy -p ... --output-format stream-json` output (agy 1.2.13,
// gemini-3.8-flash-low, 2026-09-30); `init.tools` is cut to three entries.
const PONG: &str = include_str!("../src/fixtures/antigravity-stream-json-pong.jsonl");
/// A run with a shell command, so a tool step and two agent responses appear.
const TOOL: &str = include_str!("../src/fixtures/antigravity-stream-json-tool.jsonl");
/// An unknown model: a lone `result` with `status: "ERROR"`, exit code 1.
const FAILED: &str = include_str!("../src/fixtures/antigravity-stream-json-failed.jsonl");
/// What `agy` prints on the TERM from `timeout`: `result` ERROR "interrupted".
const INTERRUPTED: &str = include_str!("../src/fixtures/antigravity-stream-json-interrupted.jsonl");
/// The tool run cut off mid-line, as a `timeout` KILL can leave it.
const TRUNCATED: &str = include_str!("../src/fixtures/antigravity-stream-json-truncated.jsonl");
const TERM_MARKER: &str = "timeout: sending signal TERM to command 'agy'\n";

fn opts() -> HeadlessOpts {
    HeadlessOpts {
        model: Some("gemini-3.8-flash-low".into()),
        high_effort: false,
        budget_usd: Some(1.0),
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
        Harness::Antigravity,
        "Reply with exactly: PONG",
        &opts(),
        Duration::from_secs(600),
    )
    .unwrap();
    (fake, result)
}

#[test]
fn argv_always_passes_the_model() {
    let argv = Harness::Antigravity
        .headless_argv("do it", &opts())
        .unwrap();
    insta::assert_debug_snapshot!(argv, @r#"
    [
        "agy",
        "-p",
        "do it",
        "--model",
        "gemini-3.8-flash-low",
        "--output-format",
        "stream-json",
        "--dangerously-skip-permissions",
    ]
    "#);
}

#[test]
fn stdin_is_closed_and_there_is_no_budget_flag() {
    assert_eq!(Harness::Antigravity.stdin(), Stdin::Closed);
    // Set on the opts above (budget_usd: Some(1.0)) and still not in the argv.
    assert_eq!(Harness::Antigravity.budget_flag(Some(1.0)), None);
    assert_eq!(Harness::Antigravity.git_repo_workaround(false), None);
}

#[test]
fn parses_a_completed_run() {
    let result = Harness::Antigravity.parse_headless_output(PONG).unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, PONG);
    assert_eq!(result.usage.input_tokens, 13093);
    assert_eq!(result.usage.output_tokens, 30);
    assert_eq!(result.usage.cost_usd, None);
}

#[test]
fn a_tool_run_sums_usage_over_the_result() {
    let result = Harness::Antigravity.parse_headless_output(TOOL).unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.answer, "DONE");
    assert_eq!(result.usage.input_tokens, 26431);
    assert_eq!(result.usage.output_tokens, 155);
}

#[test]
fn an_error_result_is_failed_with_agys_message() {
    let result = Harness::Antigravity.parse_headless_output(FAILED).unwrap();
    let RunStatus::Failed(message) = &result.status else {
        panic!("{result:?}");
    };
    assert!(message.contains("invalid model selection"), "{message}");
    assert_eq!(result.answer, "");
}

#[test]
fn a_truncated_run_keeps_the_partial_usage_and_no_result() {
    let result = Harness::Antigravity
        .parse_headless_output(TRUNCATED)
        .unwrap();
    assert!(matches!(result.status, RunStatus::Failed(_)), "{result:?}");
    assert_eq!(result.transcript, TRUNCATED);
    // The first agent_response step's usage arrived before the cut.
    assert_eq!(result.usage.input_tokens, 13111);
    assert_eq!(result.usage.output_tokens, 131);
}

#[test]
fn run_wraps_the_command_in_an_in_sandbox_timeout() {
    let (fake, result) = run_with(exec_output(PONG, "", 0));
    assert_eq!(result.status, RunStatus::Completed);
    let calls = fake.execs();
    let (_, spec) = &calls[0];
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
            "agy",
            "-p",
            "Reply with exactly: PONG",
            "--model",
            "gemini-3.8-flash-low",
            "--output-format",
            "stream-json",
            "--dangerously-skip-permissions",
        ],
        stdin: Closed,
    }
    "#);
}

#[test]
fn timeout_with_the_interrupted_result_is_timed_out() {
    // Real behavior: on TERM agy prints a result ERROR "interrupted" and
    // `timeout` exits 124. The marker, not that result, decides the status.
    let (_, result) = run_with(exec_output(INTERRUPTED, TERM_MARKER, 124));
    assert_eq!(result.status, RunStatus::TimedOut);
}

#[test]
fn timeout_with_truncated_ndjson_is_timed_out() {
    let (_, result) = run_with(exec_output(TRUNCATED, TERM_MARKER, 124));
    assert_eq!(result.status, RunStatus::TimedOut);
    assert_eq!(result.usage.input_tokens, 13111);
}

#[test]
fn the_unauthenticated_error_is_reported_as_failed() {
    // Real output of `agy -p` in a sandbox nobody signed in to (exit 1).
    let unauthenticated = "{\"event\":\"result\",\"result\":{\"conversation_id\":\"\",\"status\":\"ERROR\",\"response\":\"\",\"error\":\"authentication failed or timed out\",\"duration_seconds\":0,\"num_turns\":0,\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"thinking_tokens\":0,\"cache_read_tokens\":0,\"total_tokens\":0}}}\n";
    let (_, result) = run_with(exec_output(
        unauthenticated,
        "Error: authentication required. Run 'agy' to log in, then retry.\n",
        1,
    ));
    assert_eq!(
        result.status,
        RunStatus::Failed("authentication failed or timed out".into())
    );
}

#[test]
fn argv_omits_the_model_flag_when_absent() {
    let no_model = HeadlessOpts {
        model: None,
        ..opts()
    };
    let argv = Harness::Antigravity
        .headless_argv("do it", &no_model)
        .unwrap();
    assert!(!argv.contains(&"--model".to_owned()));
}
