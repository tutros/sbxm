use std::path::Path;
use std::time::Duration;

use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::harness::Harness;
use sbxm::headless::{self, HeadlessOpts, RunStatus, Usage};

/// Trimmed from real `claude -p ... --output-format stream-json --verbose`
/// output (Claude Code 2.1.280, 2026-09-29).
const PONG: &str = include_str!("../../src/fixtures/claude-stream-json-pong.jsonl");
/// The same run cut off mid-line, as a `timeout` KILL can leave it.
const TRUNCATED: &str = include_str!("../../src/fixtures/claude-stream-json-truncated.jsonl");
/// An error result whose subtype still says "success" (G65, issue 325): the shape and the
/// `is_error`/`subtype`/`terminal_reason`/`result` values of issue 330's worker on 2026-10-30,
/// scrubbed (ids, path, model and numbers replaced).
const IS_ERROR: &str = include_str!("../../src/fixtures/claude-stream-json-is-error.jsonl");
/// Real `timeout -v` stderr from `sbx exec` (coreutils in the sandbox).
const TERM_MARKER: &str = "timeout: sending signal TERM to command 'claude'\n";
const KILL_MARKER: &str = "timeout: sending signal TERM to command 'claude'\ntimeout: sending signal KILL to command 'claude'\n";

fn opts() -> HeadlessOpts {
    HeadlessOpts {
        model: Some("claude-haiku-4-5-20251001".into()),
        high_effort: false,
        budget_usd: None,
        is_git_repo: false,
        resume: None,
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
        RunStatus::Failed("claude reported error_max_budget_usd: PONG".into())
    );
    assert_eq!(result.answer, "PONG");
}

/// The fixture's result event with its `result` field replaced (`None` drops the field).
fn is_error_with_result(text: Option<&str>) -> String {
    let field = "\"result\":\"Credit balance is too low\",";
    assert!(IS_ERROR.contains(field));
    let replacement = match text {
        Some(text) => format!("\"result\":{},", serde_json::to_string(text).unwrap()),
        None => String::new(),
    };
    IS_ERROR.replace(field, &replacement)
}

fn failure_of(raw: &str) -> String {
    match Harness::Claude.parse_headless_output(raw).unwrap().status {
        RunStatus::Failed(why) => why,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn an_error_result_with_subtype_success_shows_the_result_text_not_success() {
    let result = Harness::Claude.parse_headless_output(IS_ERROR).unwrap();
    assert_eq!(
        result.status,
        RunStatus::Failed("claude reported an error: Credit balance is too low".into())
    );
    assert_eq!(result.answer, "Credit balance is too low");
}

#[test]
fn a_result_with_is_error_false_or_missing_is_completed() {
    let not_error = IS_ERROR.replace("\"is_error\":true", "\"is_error\":false");
    assert_eq!(
        Harness::Claude
            .parse_headless_output(&not_error)
            .unwrap()
            .status,
        RunStatus::Completed
    );
    let missing = IS_ERROR.replace("\"is_error\":true,", "");
    assert_ne!(missing, IS_ERROR);
    assert_eq!(
        Harness::Claude
            .parse_headless_output(&missing)
            .unwrap()
            .status,
        RunStatus::Completed
    );
}

#[test]
fn a_result_text_of_300_characters_is_kept_whole_and_301_is_cut() {
    let prefix = "claude reported an error: ";
    // Multi-byte characters, so a cut by bytes would show.
    let at_limit = "é".repeat(300);
    assert_eq!(
        failure_of(&is_error_with_result(Some(&at_limit))),
        format!("{prefix}{at_limit}")
    );
    let one_under = "é".repeat(299);
    assert_eq!(
        failure_of(&is_error_with_result(Some(&one_under))),
        format!("{prefix}{one_under}")
    );
    let one_over = "é".repeat(301);
    let shown = failure_of(&is_error_with_result(Some(&one_over)));
    let text = shown.strip_prefix(prefix).unwrap();
    assert_eq!(text.chars().count(), 300);
    assert_eq!(text, format!("{}…", "é".repeat(299)));
    let largest = "x".repeat(1 << 20);
    let shown = failure_of(&is_error_with_result(Some(&largest)));
    assert_eq!(shown.strip_prefix(prefix).unwrap().chars().count(), 300);
}

#[test]
fn a_multi_line_result_text_is_shown_on_one_line() {
    let shown = failure_of(&is_error_with_result(Some(
        "API Error: 401\n\n  OAuth token has expired.\r\nPlease run /login\n",
    )));
    assert_eq!(
        shown,
        "claude reported an error: API Error: 401 OAuth token has expired. Please run /login"
    );
    // A lone carriage return would also move the cursor back over the message.
    assert_eq!(
        failure_of(&is_error_with_result(Some("first\rsecond"))),
        "claude reported an error: first second"
    );
}

#[test]
fn an_empty_or_missing_result_text_is_named() {
    for raw in [
        is_error_with_result(Some("")),
        is_error_with_result(Some(" \n ")),
        is_error_with_result(None),
    ] {
        assert_eq!(
            failure_of(&raw),
            "claude reported an error with no result text"
        );
    }
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
        Harness::Pi,
        "p",
        &opts(),
        Duration::from_secs(60),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "pi has no headless adapter; use claude, codex or antigravity"
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

/// A cloud exec of a command that ignores TERM (S14 Q3, `sbx` 0.46.0): `timeout -v`'s TERM and
/// KILL lines, then the cloud's stream error, exit 1. The error line is S14's captured text; the
/// two `timeout` lines are coreutils' own wording, as above.
const CLOUD_STREAM_ERROR_AFTER_TIMEOUT: &str =
    include_str!("../../src/backend/fixtures/sbx-cloud-exec-stream-error-after-timeout.txt");
/// The same stream error with no `timeout` line before it: the cloud stopped answering.
const CLOUD_STREAM_ERROR: &str =
    include_str!("../../src/backend/fixtures/sbx-cloud-exec-stream-error.txt");

#[test]
fn the_cloud_stream_error_after_the_timeout_lines_is_timed_out() {
    let (_, result) = run_with(exec_output(TRUNCATED, CLOUD_STREAM_ERROR_AFTER_TIMEOUT, 1));
    assert_eq!(result.status, RunStatus::TimedOut);
    assert_eq!(result.answer, "PONG");
    assert_eq!(result.transcript, TRUNCATED);
}

#[test]
fn the_cloud_stream_error_without_the_timeout_lines_is_exec_unavailable() {
    let fake = FakeBackend::default().with_exec_outputs(vec![exec_output(
        TRUNCATED,
        CLOUD_STREAM_ERROR,
        1,
    )]);
    let err = headless::run(
        &fake,
        "sb",
        Path::new("/work"),
        Harness::Claude,
        "Reply with exactly: PONG",
        &opts(),
        Duration::from_secs(600),
    )
    .unwrap_err();
    let unavailable = err
        .downcast_ref::<headless::ExecUnavailable>()
        .expect("an ExecUnavailable");
    assert!(
        unavailable.0.contains("RunExecSession: stream error"),
        "{unavailable}"
    );
}

/// Issue 418: the session a Claude run used, read from its stream-json, so a fix round can resume
/// it.
#[test]
fn a_claude_stream_names_its_session() {
    let result = Harness::Claude.parse_headless_output(PONG).unwrap();
    assert_eq!(
        result.session_id.as_deref(),
        Some("92ae7cb6-403a-4abc-a295-072f335d20cb")
    );
    let truncated = Harness::Claude.parse_headless_output(TRUNCATED).unwrap();
    assert_eq!(
        truncated.session_id.as_deref(),
        Some("92ae7cb6-403a-4abc-a295-072f335d20cb")
    );
    assert_eq!(
        Harness::Claude
            .parse_headless_output("not json\n")
            .unwrap()
            .session_id,
        None
    );
}

/// Issue 418: `--resume <session>` continues a Claude session; the prompt is the next turn.
#[test]
fn claude_resumes_a_session_when_asked() {
    let resuming = HeadlessOpts {
        resume: Some("92ae7cb6-403a-4abc-a295-072f335d20cb".into()),
        ..opts()
    };
    let argv = Harness::Claude.headless_argv("fix it", &resuming).unwrap();
    assert_eq!(&argv[..3], ["claude", "-p", "fix it"]);
    let at = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[at + 1], "92ae7cb6-403a-4abc-a295-072f335d20cb");
    // Without it, no `--resume`.
    let fresh = Harness::Claude.headless_argv("fix it", &opts()).unwrap();
    assert!(!fresh.iter().any(|a| a == "--resume"));
}
