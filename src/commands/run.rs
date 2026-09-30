//! `sbxm run <config>`: validate, check secrets, reserve the run, build and
//! validate the kits, then run the pairs (milestone-2.md, "Command behavior").
//! Everything that can be refused is refused before the first write.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::headless::RunStatus;
use crate::run::config::RunConfig;
use crate::run::orchestrate::{self, PairOutcome};
use crate::run::{id, kits, preflight};

#[derive(Debug)]
pub struct Summary {
    pub run_id: String,
    pub outcomes: Vec<PairOutcome>,
}

/// Runs the comparison in `config_path`, printing the run ID and one line per
/// pair to `out` and warnings to `warn`.
pub fn run(
    config_dir: &Path,
    config_path: &Path,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<Summary> {
    let run_config = RunConfig::load(config_path)?;
    let checked = preflight::check(config_dir, &run_config, backend)?;
    for warning in &checked.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let global = GlobalConfig::load(config_dir)?;
    if !global.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            global.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }

    let roots = id::reserve(&global.base_dir)?;
    let run_kits = kits::build(config_dir, &run_config, &roots.meta.join("kits"), backend)?;
    writeln!(out, "Run {}", roots.id)?;
    let outcomes = orchestrate::execute(backend, &run_config, &run_kits, &roots);

    for outcome in &outcomes {
        let contestant = &run_config.contestants[outcome.contestant];
        let repeat = if run_config.run.repeat > 1 {
            format!(" repeat {}/{}", outcome.repeat + 1, run_config.run.repeat)
        } else {
            String::new()
        };
        writeln!(
            out,
            "contestants[{}] {}/{}{repeat}: {}",
            outcome.contestant,
            contestant.harness.as_str(),
            contestant.model,
            describe(&outcome.result)
        )?;
        if let Some(problem) = &outcome.remove_error {
            writeln!(warn, "warning: {problem}")?;
        }
        match &outcome.diff {
            Some(Err(problem)) => writeln!(
                warn,
                "warning: contestants[{}]{repeat}: {problem}",
                outcome.contestant
            )?,
            Some(Ok(diff)) if !diff.skipped.is_empty() => {
                let links: Vec<String> = diff
                    .skipped
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                writeln!(
                    warn,
                    "warning: contestants[{}]{repeat}: left links out of the diff: {}",
                    outcome.contestant,
                    links.join(", ")
                )?;
            }
            _ => {}
        }
    }
    Ok(Summary {
        run_id: roots.id,
        outcomes,
    })
}

fn describe(result: &Result<crate::headless::HeadlessResult, String>) -> String {
    match result {
        Ok(r) => match &r.status {
            RunStatus::Completed => format!(
                "completed ({} in, {} out tokens{})",
                r.usage.input_tokens,
                r.usage.output_tokens,
                r.usage
                    .cost_usd
                    .map(|c| format!(", ${c:.4}"))
                    .unwrap_or_default()
            ),
            RunStatus::TimedOut => "timed out (partial output kept)".to_owned(),
            RunStatus::Failed(why) => format!("failed: {why}"),
        },
        Err(why) => format!("error: {why}"),
    }
}
