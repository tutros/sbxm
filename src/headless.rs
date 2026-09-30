//! The shared headless-execution primitive (decision 94): runs a harness's
//! non-interactive command inside a sandbox through [`SandboxBackend::exec`],
//! with the timeout enforced inside the sandbox (decision 114).

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::backend::{ExecSpec, SandboxBackend};
use crate::harness::Harness;

/// Seconds between `timeout`'s TERM and its KILL.
const KILL_AFTER_SECS: u64 = 10;

/// What a parser reports when the stream ended without its closing event.
const NO_RESULT: &str = "no result event in the output";
const NO_TURN_COMPLETED: &str = "no turn.completed event in the output";

/// Whether the status is a failure the harness reported, not just the parser
/// noticing that the stream stopped early.
fn reported_failure(status: &RunStatus) -> bool {
    matches!(status, RunStatus::Failed(why) if why != NO_RESULT && why != NO_TURN_COMPLETED)
}

/// What [`run`] passes to the harness's command.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessOpts {
    pub model: String,
    /// Best-effort cost cap; only harnesses with a budget flag apply it.
    pub budget_usd: Option<f64>,
    /// Whether the workspace is a git repository (seeded runs); unseeded
    /// ones aren't (decision 113).
    pub is_git_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Completed,
    /// Killed by the in-sandbox `timeout`; partial output is kept.
    TimedOut,
    /// A non-timeout non-zero exit, an error result or no result at all;
    /// partial output is kept.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    /// Everything the model read: fresh, cache-written and cache-read tokens.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Only when the harness reports one natively (decision 105).
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessResult {
    pub status: RunStatus,
    pub answer: String,
    /// The raw output, kept as-is.
    pub transcript: String,
    pub usage: Usage,
}

/// Runs `prompt` headlessly in `sandbox` under `workdir`. A timeout or a
/// failing command is `Ok` with the matching status and whatever output could
/// be parsed; `Err` means the run couldn't be attempted.
pub fn run(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    workdir: &Path,
    harness: Harness,
    prompt: &str,
    opts: &HeadlessOpts,
    timeout: Duration,
) -> Result<HeadlessResult> {
    let mut argv = vec![
        "timeout".to_owned(),
        "-v".to_owned(),
        format!("--kill-after={KILL_AFTER_SECS}"),
        timeout.as_secs().max(1).to_string(),
    ];
    argv.extend(harness.headless_argv(prompt, opts)?);
    let output = backend.exec(
        sandbox,
        &ExecSpec {
            workdir: Some(workdir.to_path_buf()),
            argv,
            stdin: harness.stdin(),
        },
    )?;
    let mut result = harness.parse_headless_output(&output.stdout)?;
    // 137 is also what an OOM kill returns, so the exit code alone can't say
    // "timeout": `timeout -v` announces the signals it sends on stderr.
    let timed_out = matches!(output.exit_code, Some(124 | 137))
        && output
            .stderr
            .lines()
            .any(|line| line.starts_with("timeout: sending signal"));
    if timed_out {
        result.status = RunStatus::TimedOut;
    } else if output.exit_code != Some(0) && !reported_failure(&result.status) {
        // A failure the harness itself reported says more than stderr does, so
        // it stays; a parser's "no result event" says less than the exit code.
        let code = match output.exit_code {
            Some(code) => format!("exit code {code}"),
            None => "killed by a signal".to_owned(),
        };
        let stderr = output.stderr.trim();
        result.status = RunStatus::Failed(if stderr.is_empty() {
            code
        } else {
            format!("{code}: {stderr}")
        });
    }
    Ok(result)
}

/// Claude's `--output-format stream-json`: one JSON event per line, ending in
/// a `result` event. Best-effort on a truncated stream: unparseable lines are
/// skipped, and the last assistant text and usage seen stand in for the
/// missing `result`.
pub(crate) fn parse_claude(raw: &str) -> HeadlessResult {
    let mut answer = String::new();
    let mut usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: None,
    };
    let mut status = RunStatus::Failed(NO_RESULT.into());
    for event in raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match event["type"].as_str() {
            Some("assistant") => {
                let message = &event["message"];
                if let Some(text) = message["content"]
                    .as_array()
                    .and_then(|blocks| blocks.iter().rev().find_map(|b| b["text"].as_str()))
                {
                    answer = text.to_owned();
                }
                usage = claude_usage(&message["usage"], None);
            }
            Some("result") => {
                if let Some(text) = event["result"].as_str() {
                    answer = text.to_owned();
                }
                usage = claude_usage(&event["usage"], event["total_cost_usd"].as_f64());
                status = if event["is_error"].as_bool().unwrap_or(false) {
                    let subtype = event["subtype"].as_str().unwrap_or("error");
                    RunStatus::Failed(format!("claude reported {subtype}"))
                } else {
                    RunStatus::Completed
                };
            }
            _ => {}
        }
    }
    HeadlessResult {
        status,
        answer,
        transcript: raw.to_owned(),
        usage,
    }
}

fn claude_usage(usage: &Value, cost_usd: Option<f64>) -> Usage {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    Usage {
        input_tokens: count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens"),
        output_tokens: count("output_tokens"),
        cost_usd,
    }
}

/// Codex's `exec --json`: NDJSON with `thread.started`, `turn.started`,
/// `item.completed` (`agent_message` items carry the text; the last one is
/// the answer), `turn.completed` (usage) and `turn.failed`. An `error` *item*
/// is only a warning. Best-effort on a truncated stream, like Claude's.
pub(crate) fn parse_codex(raw: &str) -> HeadlessResult {
    let mut answer = String::new();
    let mut usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: None,
    };
    let mut status = RunStatus::Failed(NO_TURN_COMPLETED.into());
    for event in raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match event["type"].as_str() {
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                if let Some(text) = event["item"]["text"].as_str() {
                    answer = text.to_owned();
                }
            }
            Some("turn.completed") => {
                usage.input_tokens += event["usage"]["input_tokens"].as_u64().unwrap_or(0);
                usage.output_tokens += event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                status = RunStatus::Completed;
            }
            Some("turn.failed") => {
                let message = event["error"]["message"].as_str().unwrap_or("turn failed");
                status = RunStatus::Failed(message.to_owned());
            }
            _ => {}
        }
    }
    HeadlessResult {
        status,
        answer,
        transcript: raw.to_owned(),
        usage,
    }
}

/// Antigravity's `agy -p --output-format stream-json`: one JSON event per
/// line, `{"event": "init" | "step_update" | "result", ...}`. `agent_response`
/// steps carry `text_delta` and per-step `usage`; the final `result` has the
/// answer (`response`), the summed `usage` and `status` (`SUCCESS` or
/// `ERROR` with an `error` message). Best-effort on a truncated stream: the
/// last agent text and the step usages summed so far stand in for the result.
pub(crate) fn parse_antigravity(raw: &str) -> HeadlessResult {
    let mut steps: Vec<(u64, String)> = Vec::new();
    let mut steps_usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: None,
    };
    let mut result: Option<(RunStatus, String, Usage)> = None;
    for event in raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match event["event"].as_str() {
            Some("step_update") => {
                let step = &event["step_update"];
                if step["step_type"] == "agent_response" {
                    let index = step["step_index"].as_u64().unwrap_or(0);
                    if let Some(delta) = step["text_delta"].as_str() {
                        match steps.last_mut() {
                            Some((last, text)) if *last == index => text.push_str(delta),
                            _ => steps.push((index, delta.to_owned())),
                        }
                    }
                }
                if step["usage"].is_object() {
                    let usage = antigravity_usage(&step["usage"]);
                    steps_usage.input_tokens += usage.input_tokens;
                    steps_usage.output_tokens += usage.output_tokens;
                }
            }
            Some("result") => {
                let r = &event["result"];
                let status = if r["status"] == "SUCCESS" {
                    RunStatus::Completed
                } else {
                    let error = r["error"].as_str().unwrap_or("agy reported an error");
                    RunStatus::Failed(error.to_owned())
                };
                let answer = r["response"].as_str().unwrap_or("").trim_end().to_owned();
                result = Some((status, answer, antigravity_usage(&r["usage"])));
            }
            _ => {}
        }
    }
    let partial = steps
        .iter()
        .rev()
        .map(|(_, text)| text.trim_end())
        .find(|text| !text.is_empty())
        .unwrap_or("")
        .to_owned();
    let (status, answer, usage) = match result {
        Some((status, answer, usage)) => {
            // An interrupted or failed run has an empty response; keep the partial text.
            let answer = if answer.is_empty() { partial } else { answer };
            let usage = if usage.input_tokens + usage.output_tokens == 0 {
                steps_usage
            } else {
                usage
            };
            (status, answer, usage)
        }
        None => (RunStatus::Failed(NO_RESULT.into()), partial, steps_usage),
    };
    HeadlessResult {
        status,
        answer,
        transcript: raw.to_owned(),
        usage,
    }
}

fn antigravity_usage(usage: &Value) -> Usage {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    Usage {
        input_tokens: count("input_tokens") + count("cache_read_tokens"),
        output_tokens: count("output_tokens"),
        cost_usd: None,
    }
}
