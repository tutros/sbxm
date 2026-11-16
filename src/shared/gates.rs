//! The sandbox tier of a gate run (spec section 7, decision 160): commands run in a sandbox, each
//! inside the in-sandbox timeout, stopping at the first that fails. `sbxm task`'s gates
//! (`task::gates`) and `template build --warm`'s steps both run them.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::SandboxBackend;
use crate::run::checks::run_checks;
use crate::run::config::Check;

/// `[gates] timeout` when absent, and each `template build --warm` step's limit.
pub(crate) const DEFAULT_GATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Put in front of every sandbox gate command: `sbx exec` reads no login profile, so a toolchain
/// the profile installs under `~/.cargo` (Rust) isn't on the PATH without it.
pub(crate) const CARGO_ENV: &str = "[ -f \"$HOME/.cargo/env\" ] && . \"$HOME/.cargo/env\"; ";

/// One gate command's result, as `task.json` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateResult {
    pub phase: String,
    pub tier: String,
    pub command: String,
    pub exit: Option<i32>,
    pub passed: bool,
}

/// How one gate command went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub result: GateResult,
    /// The end of the command's output, for `gates.log`.
    pub output_tail: String,
    pub timed_out: bool,
    /// The command never ran at all (the sandbox `exec` call failed, the host command couldn't be
    /// started, or the clean checkout it needed couldn't be made): never inferred from `passed`,
    /// `exit` or the output text, so a timeout and an exit the shell did report (including exit
    /// 127, command not found) are never marked this way (issue 186). In memory only: `task.json`
    /// still records this the same as any other failed gate.
    pub could_not_run: bool,
}

/// Runs `commands` in `sandbox` under `workdir`, as `sh -c` inside the in-sandbox timeout (the
/// same wrapper as `sbxm run`'s checks, so a stuck command can't outlive its limit). Stops after
/// the first command that fails, times out or can't be run.
pub fn run_sandbox_tier(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    workdir: &Path,
    phase: &str,
    commands: &[String],
    timeout: Duration,
) -> Vec<GateOutcome> {
    let mut outcomes = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        let check = Check {
            id: format!("gate-{i}"),
            command: format!("{CARGO_ENV}{command}"),
            timeout: Some(timeout),
        };
        let verdict = run_checks(backend, sandbox, workdir, &[check], timeout).remove(0);
        let could_not_run = verdict.error.is_some();
        let output_tail = match &verdict.error {
            Some(error) => format!("could not run the command: {error}"),
            None if verdict.timed_out => without_timeout_lines(&verdict.output_tail),
            None => verdict.output_tail.clone(),
        };
        let passed = verdict.passed;
        outcomes.push(GateOutcome {
            result: GateResult {
                phase: phase.to_owned(),
                tier: "sandbox".to_owned(),
                command: command.clone(),
                exit: verdict.exit_code,
                passed,
            },
            output_tail,
            timed_out: verdict.timed_out,
            could_not_run,
        });
        if !passed {
            break;
        }
    }
    outcomes
}

/// `tail` without the lines `timeout -v` printed when it stopped the gate (issue 471): they are
/// sbxm's wrapper speaking, not the gate, and `gates.log` already says the gate timed out.
fn without_timeout_lines(tail: &str) -> String {
    tail.split_inclusive('\n')
        .filter(|line| !line.starts_with("timeout: sending signal"))
        .collect()
}
