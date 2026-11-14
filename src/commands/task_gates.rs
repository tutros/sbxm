//! `sbxm task gates --issue N [--tier sandbox|host|all] [--dry-run]` (spec §4, §7): runs a task's
//! gates on its current state, or with `--dry-run` says exactly what would run and where.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::harness::Harness;
use crate::run::orchestrate::in_sandbox_path;
use crate::slot;
use crate::task::config::{self, TaskConfig};
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Prepared, TaskEnv, Tiers};
use crate::task::record::{self, ProcessProbe};
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
    /// The free-space check before the worker's sandbox is made again (decision 194).
    pub disk: disk::Guard,
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
    let id = format!("issue-{}", opts.issue);
    let config = config::load_for_task(&opts.repo_root, config_dir, Some(&id))?;
    let global = GlobalConfig::load(config_dir)?;
    let base_dir = global.base_dir.clone();
    if opts.dry_run {
        return dry_run(&config, &base_dir, &id, opts, out);
    }

    // Held until the gates' last write (issue 58): a second command on this task refuses.
    let mut prepared = Prepared::open_locked(&base_dir, &id, probe)?;
    pipeline::check_can_gate(&prepared.record, probe)?;
    // The gates run where the task's record says (decision 253), whatever the config says now.
    let location = prepared.record.location;
    // A local task whose worker sandbox went when its last phase ended makes it again for the
    // gates (issue 341), so the drive of `sbx`'s sandbox state is checked first (decision 194),
    // and the machine's sandbox slot is held until the gates end (decision 304).
    let remade = location == Location::Local
        && prepared
            .record
            .worker
            .as_ref()
            .is_some_and(|w| !prepared.record.has_open_sandbox(&w.sandbox));
    if remade {
        disk::require(&global, config_dir, &opts.disk)?;
    }
    // The host tier runs in the sandbox slot too (decision 317(5)): a local task's for all its
    // gates, taken here so that a refusal comes before anything is written. A cloud task's takes
    // it in the pipeline, after any cloud slot (one order for the two pools).
    let host_tier = opts.tiers.host && !config.gates.host.is_empty();
    let _slot = if remade || (host_tier && location == Location::Local) {
        slot::take(&global, &opts.disk.slot, Some(&id), out)?
    } else {
        slot::Slot::none()
    };
    let out = RefCell::new(out);
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
        config_dir,
        config: &config,
        backend: backend.located(location),
        probe,
        host,
        sleep: &std::thread::sleep,
        // Empty in production while P8 (a) holds (slice 4).
        scoped_secrets: &[],
        disk: &opts.disk,
        identity: identity.as_ref(),
        clone_source: None,
        // Each line as it comes: a wait for the sandbox slot before a cloud task's host tier.
        progress: &|line: &str| {
            let _ = writeln!(out.borrow_mut(), "{line}");
        },
    };
    // A cloud worker sandbox found gone is made again only once a cloud slot is held (issue 599).
    let mut take_cloud_slot =
        || slot::take_cloud(&global, &opts.disk.slot, Some(&id), &mut **out.borrow_mut());
    let gated =
        pipeline::gate_on_demand_taking(&env, &mut prepared, opts.tiers, &mut take_cloud_slot);
    let out = out.into_inner();
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
            format!(
                "sandbox {}",
                record::sandbox_name(id, config.worker.harness.as_str())
            ),
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
