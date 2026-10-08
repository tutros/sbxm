//! `sbxm task resume (--issue N | --spec FILE) [--rounds N]` (issue 118; decisions 173(c), 175;
//! spec `sdlc/specs/task-state-machine.md` section 5.2): continues a task from its recorded stage,
//! keeping its clone and commits, and carries it on to `ready` like `task review`. The worker is
//! whoever the task recorded; the reviewer and the gates come from `sbxm-task.toml`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::task_review::print_review;
use super::task_start::spec_flag;
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::harness::Harness;
use crate::headless::RunStatus;
use crate::task::config::TaskConfig;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{Prepared, TaskEnv};
use crate::task::record::{self, ProcessProbe};
use crate::task::resume;

/// Which task is resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
}

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub target: Target,
    /// `--rounds N`: N more fix rounds for a task that stopped `ready`.
    pub rounds: Option<u32>,
}

impl Target {
    /// The flag that names this task again on the command line.
    fn flag(&self) -> String {
        match self {
            Self::Issue(n) => format!("--issue {n}"),
            Self::Spec(path) => spec_flag(path),
        }
    }
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if opts.target == Target::Issue(0) {
        bail!("issue numbers start at 1");
    }
    let mut config = TaskConfig::load(&opts.repo_root)?;
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = match &opts.target {
        Target::Issue(n) => format!("issue-{n}"),
        Target::Spec(path) => record::spec_id(path)?.0,
    };
    let mut prepared = Prepared::open(&base_dir, &id)?;
    // The worker is whoever the task recorded, as for `task review`: its sandbox and clone are
    // that harness's, and the default reviewer must differ from it (decision 148).
    if let Some(worker) = &prepared.record.worker {
        if let Ok(harness) = <Harness as clap::ValueEnum>::from_str(&worker.harness, true) {
            config.set_worker_harness(harness);
        }
        config.worker.model.clone_from(&worker.model);
    }
    for warning in &config.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let env = TaskEnv {
        config_dir,
        config: &config,
        backend,
        probe,
        host,
    };
    resume::check_can_resume(&prepared.record, probe, opts.rounds)?;
    writeln!(
        out,
        "{id}: resuming from {} ({})",
        prepared.record.stage.name(),
        prepared.record.status.name()
    )?;
    let resumed = resume::resume(&env, &mut prepared, opts.rounds)?;
    let flag = opts.target.flag();

    if resumed.sandbox_remade {
        writeln!(out, "{id}: the worker's sandbox was gone; made it again")?;
    }
    if let Some(worked) = &resumed.worked {
        let label = match &worked.status {
            RunStatus::Completed => "completed".to_owned(),
            RunStatus::TimedOut => "timed out".to_owned(),
            RunStatus::Failed(why) => format!("failed: {why}"),
        };
        writeln!(out, "{id}: worker {label}, {} commit(s)", worked.commits)?;
        for note in &worked.notes {
            writeln!(out, "  note: {note}")?;
        }
        if matches!(worked.status, RunStatus::Failed(_)) {
            bail!(
                "{id}: the worker failed again; see `sbxm task status` and the task's folder, \
                 then `sbxm task resume {flag}` to try once more"
            );
        }
    }
    let Some(report) = &resumed.review else {
        return Ok(());
    };
    let meta = record::task_dir(&base_dir, &id);
    if let Some(failed) = print_review(out, warn, &config, &id, &meta, report, &prepared.record)? {
        bail!(
            "gates failed for {id}: `{}`; fix it in the worker's clone and commit, then run \
             `sbxm task resume {flag}` again",
            failed.command
        );
    }
    match (report.must_fix_left, &opts.target) {
        (0, _) | (_, Target::Spec(_)) => writeln!(out, "  next: sbxm task finish {flag}")?,
        (_, Target::Issue(n)) => writeln!(
            out,
            "  next: fix them by hand, or file them: sbxm task file-findings --issue {n}"
        )?,
    }
    Ok(())
}
