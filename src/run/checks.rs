//! Executable checks (P6): each `[[eval.checks]]` command runs as `sh -c
//! <command>` inside the contestant's own sandbox, after the agent finished
//! and before the sandbox is removed, so it sees exactly the files the agent
//! left. It runs under the same in-sandbox `timeout --kill-after` as the agent
//! (decision 114) and passes when it exits 0.

use std::path::Path;
use std::time::Duration;

use super::config::Check;
use crate::backend::{ExecSpec, SandboxBackend, Stdin};
use crate::headless::hit_timeout;

/// Seconds between `timeout`'s TERM and its KILL, as for the agent.
const KILL_AFTER_SECS: u64 = 10;

/// How much of a check's combined output is kept in `evals.json`.
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// The verdict on one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub id: String,
    pub command: String,
    pub passed: bool,
    /// `None` when the command couldn't be run or was killed by a signal.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The seconds the check was allowed.
    pub timeout_secs: u64,
    /// Why the command couldn't be run at all (a backend error).
    pub error: Option<String>,
    /// The end of stdout followed by stderr, for debugging a failure.
    pub output_tail: String,
}

/// Runs `checks` one after another in `sandbox` and returns a verdict for
/// each. A check that hangs, fails or can't be run never stops the next one.
/// `default_timeout` applies to checks without their own `timeout`.
pub fn run_checks(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    workdir: &Path,
    checks: &[Check],
    default_timeout: Duration,
) -> Vec<CheckOutcome> {
    checks
        .iter()
        .map(|check| {
            let timeout_secs = check.timeout.unwrap_or(default_timeout).as_secs().max(1);
            let spec = ExecSpec {
                workdir: Some(workdir.to_path_buf()),
                argv: [
                    "timeout",
                    "-v",
                    &format!("--kill-after={KILL_AFTER_SECS}"),
                    &timeout_secs.to_string(),
                    "sh",
                    "-c",
                    &check.command,
                ]
                .map(str::to_owned)
                .into(),
                stdin: Stdin::Closed,
            };
            let mut outcome = CheckOutcome {
                id: check.id.clone(),
                command: check.command.clone(),
                passed: false,
                exit_code: None,
                timed_out: false,
                timeout_secs,
                error: None,
                output_tail: String::new(),
            };
            match backend.exec(sandbox, &spec) {
                Ok(output) => {
                    outcome.passed = output.exit_code == Some(0);
                    outcome.exit_code = output.exit_code;
                    outcome.timed_out = hit_timeout(output.exit_code, &output.stderr);
                    outcome.output_tail = tail(
                        &format!("{}{}", output.stdout, output.stderr),
                        OUTPUT_TAIL_BYTES,
                    );
                }
                Err(e) => outcome.error = Some(format!("{e:#}")),
            }
            outcome
        })
        .collect()
}

/// The last `max` bytes of `text` (rounded up to a character boundary).
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}
