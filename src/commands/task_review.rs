//! `sbxm task review --issue N` (spec §4, §5.2): gates, an independent reviewer in its own
//! sandbox, at most one fix round, then `ready`. Findings left over are reported, not an error.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::harness::Harness;
use crate::run::config::{headless_harness, parse_duration};
use crate::task::config::TaskConfig;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Prepared, TaskEnv};
use crate::task::record::{self, ProcessProbe};

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub issue: u32,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    pub reviewer_time_limit: Option<String>,
    /// The worker's time limit for the fix round.
    pub time_limit: Option<String>,
    pub profile: Option<String>,
}

pub fn run(
    config_dir: &std::path::Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if opts.issue == 0 {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    // Every input is checked before anything is created.
    let mut config = TaskConfig::load(&opts.repo_root)?;
    if let Some(harness) = opts.reviewer_harness {
        let harness = headless_harness("--reviewer-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
        config.set_reviewer_harness(harness);
    }
    if let Some(model) = &opts.reviewer_model {
        config.reviewer.model = Some(model.clone());
    }
    if let Some(limit) = &opts.reviewer_time_limit {
        config.reviewer.time_limit =
            parse_duration("--reviewer-time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(limit) = &opts.time_limit {
        config.worker.time_limit = parse_duration("--time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(profile) = &opts.profile {
        config.sandbox.profile = profile.clone();
    }
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = format!("issue-{}", opts.issue);
    let mut prepared = Prepared::open(&base_dir, &id)?;

    let env = TaskEnv {
        config_dir,
        config: &config,
        backend,
        probe,
        host,
    };
    // The same-harness warning is wanted even if the review then fails, so say it up front.
    for warning in &config.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let report = pipeline::review_issue(&env, &mut prepared)?;
    for warning in report
        .warnings
        .iter()
        .filter(|w| !config.warnings.contains(w))
    {
        writeln!(warn, "warning: {warning}")?;
    }

    let meta = record::task_dir(&base_dir, &id);
    if let Some(failed) = &report.gates_failed {
        let exit = failed
            .exit
            .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
        let log = meta.join("gates.log");
        writeln!(
            out,
            "{id}: gates failed: `{}` ({}, {exit}, {}); output in {}",
            failed.command,
            failed.tier,
            failed.phase,
            log.display()
        )?;
        for round in &report.rounds {
            writeln!(
                out,
                "{id}: review round {}: {} must-fix finding(s)",
                round.round, round.must_fix
            )?;
        }
        bail!(
            "gates failed for {id}: `{}`; fix it, then run `sbxm task gates --issue {}` and review again",
            failed.command,
            opts.issue
        );
    }

    for round in &report.rounds {
        writeln!(
            out,
            "{id}: review round {}: {} must-fix finding(s)",
            round.round, round.must_fix
        )?;
        if round.round == 1 && report.fix_ran {
            writeln!(out, "{id}: fix round ran; the gates after it passed")?;
        }
    }
    let review = meta.join("review.md");
    if report.must_fix_left == 0 {
        writeln!(out, "{id}: ready; the review is {}", review.display())?;
        writeln!(out, "  next: sbxm task finish --issue {}", opts.issue)?;
    } else {
        writeln!(
            out,
            "{id}: ready, with {} must-fix finding(s) left; read {}",
            report.must_fix_left,
            review.display()
        )?;
        writeln!(
            out,
            "  next: fix them by hand, or file them: sbxm task file-findings --issue {}",
            opts.issue
        )?;
    }
    Ok(())
}
