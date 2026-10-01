//! Gates (spec §7, decision 160): commands that decide whether a task's work may go on. They run
//! as a build step by sbxm, never left to the agent's prompt. Two tiers: the *sandbox* tier in the
//! worker's own sandbox, and an optional *host* tier on this machine, each stopping at the first
//! failing command.

use std::path::Path;
use std::time::Duration;

use super::record::GateResult;
use crate::backend::SandboxBackend;
use crate::run::checks::run_checks;
use crate::run::config::Check;

/// How one gate command went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub result: GateResult,
    /// The end of the command's output, for `gates.log`.
    pub output_tail: String,
    pub timed_out: bool,
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
            command: command.clone(),
            timeout: Some(timeout),
        };
        let verdict = run_checks(backend, sandbox, workdir, &[check], timeout).remove(0);
        let output_tail = match &verdict.error {
            Some(error) => format!("could not run the command: {error}"),
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
        });
        if !passed {
            break;
        }
    }
    outcomes
}
