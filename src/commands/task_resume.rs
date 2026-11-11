//! `sbxm task resume (--issue N | --spec FILE | --pr N) [--rounds N [--should-fix]]` (issue 118;
//! decisions 173(c), 175; spec `sdlc/specs/task-state-machine.md` section 5.2): continues a task
//! from its recorded stage, keeping its clone and commits, and carries it on to `ready` like `task
//! review`. `--should-fix` (issue 510, decision 306) gives a clean `ready` task one fix round on
//! its review's should-fix findings sized S.
//! The worker is whoever the task recorded; the reviewer and the gates come from `sbxm-task.toml`.
//! `--pr N` (issue 485) resumes the `task fix --pr N` task of the PR's current head and, as
//! `task fix` does, pushes its result onto the PR's branch once its gates pass.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::task_fix::print_pushed;
use super::task_log;
use super::task_review::{print_review, resume_hint};
use super::task_start::spec_flag;
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::headless::RunStatus;
use crate::slot;
use crate::task::config;
use crate::task::finish;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Prepared, TaskEnv};
use crate::task::record::{self, ProcessProbe};
use crate::task::repo::Identity;
use crate::task::resume;

/// Which task is resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
    /// The `task fix --pr N` task of PR N's current head (issue 485).
    Pr(u32),
}

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub target: Target,
    /// `--rounds N`: N more fix rounds for a task that stopped `ready`.
    pub rounds: Option<u32>,
    /// `--should-fix` (issue 510, decision 306): the first of those rounds, on a clean `ready`
    /// task, works on its review's should-fix findings sized S only.
    pub should_fix: bool,
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
    /// For tests: the committer identity a cloud worker sandbox made again gives its clone,
    /// instead of the user's global git config (milestone 4 slice 6).
    pub identity: Option<Identity>,
    /// `--forget-ambiguous` (decision 268): drop a cloud create that was started and never
    /// recorded as returned from the record, touching nothing in the cloud for it, then resume.
    pub forget_ambiguous: bool,
}

impl Target {
    /// The flag that names this task again on the command line.
    fn flag(&self) -> String {
        match self {
            Self::Issue(n) => format!("--issue {n}"),
            Self::Spec(path) => spec_flag(path),
            Self::Pr(n) => format!("--pr {n}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if opts.target == Target::Issue(0) {
        bail!("issue numbers start at 1");
    }
    if opts.target == Target::Pr(0) {
        bail!("PR numbers start at 1");
    }
    let named = match &opts.target {
        Target::Issue(n) => Some(format!("issue-{n}")),
        Target::Spec(path) => record::spec_id(path).ok().map(|(id, _)| id),
        // The fix task of the PR's current head is only known once GitHub is asked.
        Target::Pr(_) => None,
    };
    let mut config = config::load_for_task(&opts.repo_root, config_dir, named.as_deref())?;
    let global = GlobalConfig::load(config_dir)?;
    let base_dir = global.base_dir.clone();
    let id = match &opts.target {
        Target::Issue(n) => format!("issue-{n}"),
        Target::Spec(path) => record::spec_id(path)?.0,
        Target::Pr(n) => resume::fix_task_for_head(&base_dir, *n, github)?,
    };
    let mut prepared = Prepared::open_locked(&base_dir, &id, probe)?;
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
    // The task resumes where its record says (decision 253), whatever the config says now; a
    // cloud one may make its worker sandbox again, whose clone needs a committer identity.
    let location = prepared.record.location;
    let identity = match (location, &opts.identity) {
        (Location::Local, _) => None,
        (Location::Cloud, Some(identity)) => Some(identity.clone()),
        (Location::Cloud, None) => Identity::read_from(None).ok(),
    };
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
        progress: &pipeline::quiet,
    };
    resume::check_can_resume_with(&prepared.record, probe, opts.rounds, opts.should_fix)?;
    resume::check_ambiguous(&prepared.record, opts.forget_ambiguous)?;
    let small = if opts.should_fix {
        resume::check_should_fix(&prepared.record, &prepared.meta, opts.rounds)?
    } else {
        Vec::new()
    };
    // A cloud sandbox doesn't use the drive of `sbx`'s local sandbox state (P11, decision
    // 262(h)).
    // Held until the resumed task ends (decision 304); a cloud task takes none.
    let _slot = match location {
        Location::Local => {
            disk::require(&global, config_dir, &opts.disk)?;
            slot::take(&global, &opts.disk.slot, Some(&id), warn)?
        }
        Location::Cloud => slot::Slot::none(),
    };
    if !opts.should_fix && resume::only_push_left(&prepared.record) {
        writeln!(
            out,
            "{id}: resuming from ready: its push onto PR #{}'s branch",
            prepared.record.number
        )?;
        return push_fix(&base_dir, &id, &prepared, github, probe, out);
    }
    writeln!(
        out,
        "{id}: resuming from {} ({})",
        prepared.record.stage.name(),
        prepared.record.status.name()
    )?;
    if let Some(n) = opts.rounds {
        writeln!(
            out,
            "{id}: {n} more fix round(s), {} in all (stopped: {})",
            resume::rounds_cap(&prepared.record, n).unwrap_or(u32::MAX),
            prepared.record.stopped.map_or("", record::Stopped::name)
        )?;
    }
    if !small.is_empty() {
        let ids: Vec<&str> = small.iter().map(|f| f.id.as_str()).collect();
        writeln!(
            out,
            "{id}: a should-fix round on its review's should-fix findings sized S: {}",
            ids.join(", ")
        )?;
    }
    let resumed = task_log::live(out, |progress| {
        resume::resume_forgetting(
            &TaskEnv { progress, ..env },
            &mut prepared,
            opts.rounds,
            opts.forget_ambiguous,
            opts.should_fix,
        )
    });
    // What the cloud sandboxes did (their creates and removals, decision 265) and a remade
    // sandbox's warnings (a template it couldn't use), even when the resume then failed.
    for line in prepared.events.drain(..) {
        writeln!(out, "{line}")?;
    }
    for warning in &prepared.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let resumed = resumed?;
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
        writeln!(
            out,
            "{id}: worker {} {label}, {} commit(s)",
            worked.agent, worked.commits
        )?;
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
        match &report.gates_could_not_run {
            Some(reason) => bail!(
                "gates for {id} could not run: `{}`; {reason}; see {}",
                failed.command,
                meta.join("gates.log").display()
            ),
            None => bail!(
                "gates failed for {id}: `{}`; fix it in the worker's clone and commit, then run \
                 `sbxm task resume {flag}` again",
                failed.command
            ),
        }
    }
    let comment = crate::title::hint_comment(&prepared.record);
    match (report.must_fix_left, &opts.target) {
        // A fix task is pushed as `task fix` pushes it, must-fix findings left or not.
        (_, Target::Pr(_)) => return push_fix(&base_dir, &id, &prepared, github, probe, out),
        (0, _) | (_, Target::Spec(_)) => {
            writeln!(out, "  next: sbxm task finish {flag}{comment}")?;
        }
        (_, Target::Issue(n)) => writeln!(
            out,
            "  next: fix them by hand, or file them: sbxm task file-findings --issue {n}{comment}"
        )?,
    }
    if let Some(hint) = resume_hint(&prepared.record, &flag) {
        writeln!(out, "{hint}")?;
    }
    Ok(())
}

/// The end of a fix task's resume (issue 485), as of `task fix`: once the worker merged the base
/// (decision 292), its result is pushed onto the PR's branch as a fast-forward, and the in-task
/// review posted as the new head's.
fn push_fix(
    base_dir: &Path,
    id: &str,
    prepared: &Prepared,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    pipeline::check_base_merged(prepared)?;
    let pushed = finish::finish_fix(base_dir, id, github, probe)?;
    print_pushed(
        out,
        id,
        prepared.record.number,
        &prepared.record.repo,
        &pushed,
    )
}
