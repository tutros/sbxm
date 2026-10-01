//! `sbxm task gates --issue N [--tier sandbox|host|all] [--dry-run]` (spec §4, §7): runs a task's
//! gates on its current state, or with `--dry-run` says exactly what would run and where.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::run::orchestrate::in_sandbox_path;
use crate::task::config::TaskConfig;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Prepared, TaskEnv, Tiers};
use crate::task::record::ProcessProbe;

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub issue: u32,
    pub tiers: Tiers,
    pub dry_run: bool,
}

fn limit(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn numbered(commands: &[String]) -> String {
    commands
        .iter()
        .enumerate()
        .map(|(i, c)| format!("  {}. {c}\n", i + 1))
        .collect()
}

pub fn run(
    config_dir: &std::path::Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
) -> Result<()> {
    if opts.issue == 0 {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    let config = TaskConfig::load(&opts.repo_root)?;
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = format!("issue-{}", opts.issue);
    if opts.dry_run {
        return dry_run(&config, &base_dir, &id, opts, out);
    }

    let mut prepared = Prepared::open(&base_dir, &id)?;
    pipeline::check_can_gate(&prepared.record, probe)?;
    let env = TaskEnv {
        config_dir,
        config: &config,
        backend,
        probe,
        host,
    };
    let gated = pipeline::run_gates(&env, &mut prepared, "on-demand", opts.tiers)?;
    let log = prepared.meta.join("gates.log");
    if gated.passed {
        let count = |tier: &str| {
            gated
                .outcomes
                .iter()
                .filter(|o| o.result.tier == tier)
                .count()
        };
        if gated.outcomes.is_empty() {
            writeln!(
                out,
                "{id}: no gates ran (none configured for the chosen tier); log: {}",
                log.display()
            )?;
        } else {
            writeln!(
                out,
                "{id}: gates passed ({} sandbox, {} host); log: {}",
                count("sandbox"),
                count("host"),
                log.display()
            )?;
        }
        return Ok(());
    }
    let first = gated.failed.expect("failed gates name the first failure");
    let exit = first
        .exit
        .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
    bail!(
        "gates failed for {id}: `{}` ({}, {exit}); output in {}",
        first.command,
        first.tier,
        log.display()
    )
}

fn dry_run(
    config: &TaskConfig,
    base_dir: &std::path::Path,
    id: &str,
    opts: &Options,
    out: &mut dyn Write,
) -> Result<()> {
    let gates = &config.gates;
    let timeout = limit(gates.timeout);
    let sandbox = match Prepared::open(base_dir, id)
        .ok()
        .and_then(|p| p.record.worker)
    {
        Some(worker) => worker.sandbox,
        None => format!("sbxm-task-{id}-{}", config.worker.harness.as_str()),
    };
    let workspace = base_dir.join("tasks").join(id);
    writeln!(out, "Gates for {id} (dry run: nothing is run or changed)")?;

    if !opts.tiers.sandbox {
        writeln!(out, "sandbox tier: not selected (--tier host)")?;
    } else if gates.sandbox.is_empty() {
        writeln!(
            out,
            "sandbox tier: no commands (an empty list means no sandbox gates)"
        )?;
    } else {
        writeln!(
            out,
            "sandbox tier: in sandbox {sandbox}, in {}, each command limited to {timeout}, stops at the first failure:\n{}",
            in_sandbox_path(&workspace).display(),
            numbered(&gates.sandbox).trim_end()
        )?;
    }

    if !opts.tiers.host {
        writeln!(out, "host tier: not selected (--tier sandbox)")?;
    } else if gates.host.is_empty() {
        writeln!(
            out,
            "host tier: off (no commands under [gates] host in {})",
            crate::task::config::FILE_NAME
        )?;
    } else {
        writeln!(
            out,
            "host tier: on this machine, in a clean checkout of {id} at {}, run only after the sandbox tier passes, \
             each command limited to {timeout}. This runs agent-written code on this machine:\n{}",
            workspace.with_file_name(format!("{id}-gates")).display(),
            numbered(&gates.host).trim_end()
        )?;
    }
    Ok(())
}
