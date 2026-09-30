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

/// What [`run`] passes to the harness's command.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessOpts {
    pub model: String,
    /// Best-effort cost cap; only harnesses with a budget flag apply it.
    pub budget_usd: Option<f64>,
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
    } else if output.exit_code != Some(0) {
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
    let mut status = RunStatus::Failed("no result event in the output".into());
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
