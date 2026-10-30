//! `sbxm task gates --issue N [--tier sandbox|host|all] [--dry-run]` (spec §4, §7): runs a task's
//! gates on its current state, or with `--dry-run` says exactly what would run and where.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::harness::Harness;
use crate::run::orchestrate::in_sandbox_path;
use crate::task::config::TaskConfig;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Prepared, TaskEnv, Tiers};
use crate::task::record::ProcessProbe;
use crate::task::repo::Identity;

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub issue: u32,
    pub tiers: Tiers,
    pub dry_run: bool,
    /// For tests: the committer identity a cloud worker sandbox made again for the gates gives
    /// its clone, instead of the user's global git config (milestone 4 slice 6).
    pub identity: Option<Identity>,
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
    // The gates run where the task's record says (decision 253), whatever the config says now.
    let location = prepared.record.location;
    let identity = match (location, &opts.identity) {
        (Location::Local, _) => None,
        (Location::Cloud, Some(identity)) => Some(identity.clone()),
        (Location::Cloud, None) => Identity::read_from(None).ok(),
    };
    // The worker is whoever the task recorded, as for `task resume`: a cloud sandbox made again
    // for the gates is that harness's.
    let mut config = config;
    if let Some(worker) = &prepared.record.worker
        && let Ok(harness) = <Harness as clap::ValueEnum>::from_str(&worker.harness, true)
    {
        config.set_worker_harness(harness);
    }
    let env = TaskEnv {
        auth: &crate::auth::Host::real(),
        config_dir,
        config: &config,
        backend: backend.located(location),
        probe,
        host,
        sleep: &std::thread::sleep,
        // Empty in production while P8 (a) holds (slice 4).
        scoped_secrets: &[],
        identity: identity.as_ref(),
    };
    let gated = pipeline::gate_on_demand(&env, &mut prepared, opts.tiers);
    for line in prepared.events.drain(..) {
        writeln!(out, "{line}")?;
    }
    for warning in prepared.warnings.drain(..) {
        writeln!(out, "warning: {warning}")?;
    }
    let gated = gated?;
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
    let recorded = Prepared::open(base_dir, id).ok().map(|p| p.record);
    let workspace = base_dir.join("tasks").join(id);
    // Where the sandbox tier runs: the recorded worker's sandbox and, for a task recorded in the
    // cloud, its recorded clone (milestone 4 slice 6), else the mounted workspace.
    let (sandbox, clone) = match recorded
        .as_ref()
        .and_then(|r| r.worker.as_ref().map(|w| (r, w)))
    {
        Some((task, worker)) if task.location == Location::Cloud => (
            format!("cloud sandbox {}", worker.sandbox),
            PathBuf::from(&worker.workspace),
        ),
        Some((_, worker)) => (
            format!("sandbox {}", worker.sandbox),
            in_sandbox_path(&workspace),
        ),
        None => (
            format!("sandbox sbxm-task-{id}-{}", config.worker.harness.as_str()),
            in_sandbox_path(&workspace),
        ),
    };
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
            "sandbox tier: in {sandbox}, in {}, each command limited to {timeout}, stops at the first failure:\n{}",
            clone.display(),
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
