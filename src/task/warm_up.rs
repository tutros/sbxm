//! The warm-up of a strict cloud sandbox (issue 560). A cloud sandbox has no warm template
//! (decision 254), so its cargo registry is empty when sbxm denies the build hosts (decisions
//! 255, 264). Before that deny, while the build hosts are still allowed, the clone is checked out
//! at the task's base commit, detached, and the same steps as `sbxm template build --warm` run in
//! it ([`template_build::warm_steps`]): the repo's `[warm]` commands, else, for a repo whose base
//! has a `Cargo.toml`, the built-in cargo steps. Only the base is ever built while the build hosts
//! are allowed; the task's branch is checked out after the deny. A step that fails, or the whole
//! warm-up past [`LIMIT`], fails the task before any agent runs, on the path a failed strict check
//! takes.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::config::Warm;
use super::gates::{self, GateOutcome};
use crate::backend::SandboxBackend;
use crate::commands::template_build::{self, WarmStep};

/// The whole warm-up's time limit, enforced inside the sandbox: each step's `timeout` is what is
/// left of it.
pub const LIMIT: Duration = Duration::from_secs(20 * 60);

/// How many lines of a failed step's output its error keeps.
pub const TAIL_LINES: usize = 40;

/// The error [`run`] returns: a step that failed, couldn't run or ran out of time. Its own type, so
/// a first start that fails here keeps its task for `task resume` (review M-1 of issue 560).
#[derive(Debug)]
pub struct Failed(String);

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failed {}

/// Whether `err` is, or was caused by, a warm-up that failed.
pub fn failed(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<Failed>())
}

/// The steps a strict cloud sandbox warms up with: the task config's `[warm]` commands, else the
/// built-in cargo steps when the base commit `base` of `repo_git` has a `Cargo.toml`, else none.
pub fn steps(warm: Option<&Warm>, repo_git: &Path, base: &str) -> Result<Vec<WarmStep>> {
    if let Some(warm) = warm {
        return Ok(template_build::warm_steps(Some(&warm.commands)));
    }
    let manifest = format!("{base}:Cargo.toml");
    let found = crate::git::check_objects(repo_git, Some(repo_git), None, &[&manifest])?;
    Ok(match found.first() {
        Some(Some(object)) if object.kind == "blob" => template_build::warm_steps(None),
        _ => Vec::new(),
    })
}

/// Runs `steps` in order in `sandbox`'s clone `clone`, each under the in-sandbox timeout of what
/// is left of `limit`. The first that fails, can't run or runs out of time is the error: the step
/// named, and the last [`TAIL_LINES`] lines of its output.
pub fn run(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    clone: &str,
    steps: &[WarmStep],
    limit: Duration,
) -> Result<()> {
    let started = Instant::now();
    for (i, step) in steps.iter().enumerate() {
        let which = format!("{} of {} `{}`", i + 1, steps.len(), step.name);
        let left = limit.saturating_sub(started.elapsed());
        if left < Duration::from_secs(1) {
            return Err(Failed(format!(
                "warm-up timed out after {} in strict cloud sandbox {sandbox} before step \
                 {which}; no agent ran",
                minutes(limit)
            ))
            .into());
        }
        let outcome = gates::run_sandbox_tier(
            backend,
            sandbox,
            Path::new(clone),
            "warm",
            std::slice::from_ref(&step.command),
            left,
        )
        .remove(0);
        if !outcome.result.passed {
            return Err(step_failed(sandbox, &which, &outcome, limit).into());
        }
    }
    Ok(())
}

/// Why step `which` stopped the warm-up, with the end of its output.
fn step_failed(sandbox: &str, which: &str, outcome: &GateOutcome, limit: Duration) -> Failed {
    let what = if outcome.timed_out {
        format!("warm-up timed out after {} at step {which}", minutes(limit))
    } else if outcome.could_not_run {
        format!("warm-up step {which} could not run")
    } else {
        match outcome.result.exit {
            Some(code) => format!("warm-up step {which} failed with exit {code}"),
            None => format!("warm-up step {which} failed"),
        }
    };
    Failed(format!(
        "{what} in strict cloud sandbox {sandbox}; no agent ran (`sbxm task resume` makes the \
         sandbox afresh); its output ends:\n{}",
        last_lines(&outcome.output_tail, TAIL_LINES)
    ))
}

/// `20m` for twenty minutes.
fn minutes(limit: Duration) -> String {
    format!("{}m", limit.as_secs() / 60)
}

/// The last `n` lines of `text`.
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_keeps_at_most_n_and_all_of_fewer() {
        let text: String = (1..=41).map(|i| format!("line {i}\n")).collect();
        let kept = last_lines(&text, 40);
        assert_eq!(kept.lines().count(), 40);
        assert!(kept.starts_with("line 2\n") && kept.ends_with("line 41"));
        assert_eq!(last_lines("a\nb\n", 40), "a\nb");
        assert_eq!(last_lines("", 40), "");
    }
}
